use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_loadtools::{
    fetch_prometheus_samples, find_metric_value_u64, run_http_load_probe, HttpLoadProbeConfig,
    HttpLoadProbeResponseMode, HttpLoadProbeResult, PrometheusSample,
};
use reqwest::Method;
use serde::Serialize;
use tokio::sync::Mutex;

const DB_POOL_PRESSURE_USAGE_BASIS_POINTS: u64 = 9_000;
const MAX_DB_POOL_PRESSURE_WINDOWS: usize = 32;

#[derive(Debug, Clone)]
struct Config {
    load: HttpLoadProbeConfig,
    metrics_url: String,
    sample_interval: Duration,
    settle_after: Duration,
    output_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct GatewayPressureReport {
    suite: &'static str,
    target_url: String,
    metrics_url: String,
    sample_interval_ms: u64,
    settle_after_ms: u64,
    settle_drain_completed: bool,
    settle_drain_elapsed_ms: u64,
    load: HttpLoadProbeResult,
    metrics: GatewayPressureMetricsSummary,
}

#[derive(Debug, Clone, Default, Serialize)]
struct GatewayPressureMetricsSummary {
    samples: usize,
    db_pool_max_checked_out: u64,
    db_pool_min_idle: Option<u64>,
    db_pool_max_size: u64,
    db_pool_max_connections: u64,
    db_pool_max_usage_basis_points: u64,
    db_pool_max_idle_reserve: u64,
    db_pool_pressure_samples: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    db_pool_pressure_windows: Vec<GatewayDbPoolPressureWindow>,
    postgres_observability_available: u64,
    postgres_observability_unavailable_samples: usize,
    postgres_max_active_connections: u64,
    postgres_final_active_connections: u64,
    postgres_max_waiting_connections: u64,
    postgres_final_waiting_connections: u64,
    postgres_max_lock_waiting_connections: u64,
    postgres_final_lock_waiting_connections: u64,
    postgres_max_idle_in_transaction_connections: u64,
    postgres_final_idle_in_transaction_connections: u64,
    postgres_max_oldest_active_query_age_ms: u64,
    postgres_final_oldest_active_query_age_ms: u64,
    postgres_max_oldest_transaction_age_ms: u64,
    postgres_final_oldest_transaction_age_ms: u64,
    postgres_deadlocks_total_final: u64,
    postgres_max_block_cache_hit_rate_basis_points: u64,
    postgres_final_block_cache_hit_rate_basis_points: u64,
    postgres_final_temp_bytes_total: u64,
    postgres_final_xact_rollback_total: u64,
    postgres_wal_observability_available: u64,
    postgres_wal_observability_unavailable_samples: usize,
    postgres_final_wal_bytes_total: u64,
    postgres_final_wal_write_time_ms_total: u64,
    postgres_final_wal_sync_time_ms_total: u64,
    postgres_checkpoint_observability_available: u64,
    postgres_checkpoint_observability_unavailable_samples: usize,
    postgres_final_checkpoint_write_time_ms_total: u64,
    postgres_final_checkpoint_sync_time_ms_total: u64,
    postgres_final_buffers_checkpoint_total: u64,
    postgres_final_buffers_backend_total: u64,
    postgres_statement_observability_available: u64,
    postgres_statement_observability_unavailable_samples: usize,
    postgres_max_statement_top_max_mean_exec_time_ms: u64,
    postgres_max_statement_top_max_exec_time_ms: u64,
    postgres_first_statement_top_max_exec_time_ms: Option<u64>,
    postgres_final_statement_top_max_exec_time_ms: u64,
    postgres_statement_top_max_exec_time_ms_delta: u64,
    postgres_final_statement_top_exec_time_ms_total: u64,
    postgres_final_statement_top_temp_blks_total: u64,
    redis_runtime_enabled: u64,
    redis_runtime_health_unavailable_samples: usize,
    redis_runtime_max_connected_clients: u64,
    redis_runtime_final_connected_clients: u64,
    redis_runtime_max_blocked_clients: u64,
    redis_runtime_final_blocked_clients: u64,
    redis_runtime_max_used_memory_bytes: u64,
    redis_runtime_final_used_memory_bytes: u64,
    redis_runtime_max_memory_usage_basis_points: u64,
    redis_runtime_final_memory_usage_basis_points: u64,
    redis_runtime_max_memory_fragmentation_ratio_basis_points: u64,
    redis_runtime_max_instantaneous_ops_per_sec: u64,
    redis_runtime_max_rejected_connections_total: u64,
    redis_runtime_first_rejected_connections_total: Option<u64>,
    redis_runtime_final_rejected_connections_total: u64,
    redis_runtime_rejected_connections_total_delta: u64,
    redis_runtime_max_evicted_keys_total: u64,
    redis_runtime_first_evicted_keys_total: Option<u64>,
    redis_runtime_final_evicted_keys_total: u64,
    redis_runtime_evicted_keys_total_delta: u64,
    redis_runtime_max_total_error_replies: u64,
    redis_runtime_first_total_error_replies: Option<u64>,
    redis_runtime_final_total_error_replies: u64,
    redis_runtime_total_error_replies_delta: u64,
    redis_runtime_max_lane_command_errors_total: u64,
    redis_runtime_first_lane_command_errors_total: Option<u64>,
    redis_runtime_final_lane_command_errors_total: u64,
    redis_runtime_lane_command_errors_total_delta: u64,
    redis_runtime_max_lane_command_timeouts_total: u64,
    redis_runtime_first_lane_command_timeouts_total: Option<u64>,
    redis_runtime_final_lane_command_timeouts_total: u64,
    redis_runtime_lane_command_timeouts_total_delta: u64,
    redis_runtime_max_command_count_total: u64,
    redis_runtime_max_command_latency_ms: u64,
    redis_runtime_max_nonblocking_command_latency_ms: u64,
    redis_runtime_first_nonblocking_command_latency_ms: Option<u64>,
    redis_runtime_final_nonblocking_command_latency_ms: u64,
    redis_runtime_nonblocking_command_latency_ms_delta: u64,
    redis_runtime_first_nonblocking_command_count_total: Option<u64>,
    redis_runtime_final_nonblocking_command_count_total: u64,
    redis_runtime_nonblocking_command_count_total_delta: u64,
    redis_runtime_first_nonblocking_command_le_500ms_total: Option<u64>,
    redis_runtime_final_nonblocking_command_le_500ms_total: u64,
    redis_runtime_nonblocking_command_le_500ms_total_delta: u64,
    redis_runtime_nonblocking_command_over_500ms_total_delta: u64,
    redis_runtime_nonblocking_command_over_500ms_rate_basis_points: u64,
    redis_runtime_max_fast_command_latency_ms: u64,
    redis_runtime_max_stream_command_latency_ms: u64,
    redis_runtime_max_admin_command_latency_ms: u64,
    redis_runtime_max_blocking_stream_command_latency_ms: u64,
    gateway_requests_max_in_flight: u64,
    gateway_requests_max_rejected_total: u64,
    gateway_requests_distributed_max_in_flight: u64,
    gateway_requests_distributed_max_rejected_total: u64,
    request_candidate_queue_max_depth: u64,
    request_candidate_queue_final_depth: u64,
    request_candidate_queue_max_pending_depth: u64,
    request_candidate_queue_final_pending_depth: u64,
    request_candidate_queue_capacity: u64,
    request_candidate_queue_max_enqueued_total: u64,
    request_candidate_queue_max_flushed_total: u64,
    request_candidate_queue_max_flush_batches_total: u64,
    request_candidate_queue_max_flush_sql_ops_total: u64,
    request_candidate_queue_max_flush_sql_records_total: u64,
    request_candidate_queue_max_db_write_concurrency_limit: u64,
    request_candidate_queue_max_db_write_max_in_flight: u64,
    request_candidate_queue_max_db_write_wait_total: u64,
    request_candidate_queue_max_compacted_total: u64,
    request_candidate_queue_max_dropped_total: u64,
    request_candidate_queue_max_flush_failed_total: u64,
    request_candidate_queue_max_sync_fallback_total: u64,
    usage_runtime_max_terminal_enqueue_failed_total: u64,
    usage_runtime_max_lifecycle_enqueue_failed_total: u64,
    usage_runtime_max_lifecycle_enqueue_deferred_dropped_total: u64,
    usage_runtime_max_enqueue_retry_pending: u64,
    usage_runtime_final_enqueue_retry_pending: u64,
    usage_runtime_first_enqueue_retry_scheduled_total: Option<u64>,
    usage_runtime_final_enqueue_retry_scheduled_total: u64,
    usage_runtime_enqueue_retry_scheduled_total_delta: u64,
    usage_runtime_first_enqueue_retry_recovered_total: Option<u64>,
    usage_runtime_final_enqueue_retry_recovered_total: u64,
    usage_runtime_enqueue_retry_recovered_total_delta: u64,
    usage_runtime_first_enqueue_retry_failed_total: Option<u64>,
    usage_runtime_final_enqueue_retry_failed_total: u64,
    usage_runtime_enqueue_retry_failed_total_delta: u64,
    usage_runtime_first_enqueue_retry_closed_or_unavailable_total: Option<u64>,
    usage_runtime_final_enqueue_retry_closed_or_unavailable_total: u64,
    usage_runtime_enqueue_retry_closed_or_unavailable_total_delta: u64,
    usage_runtime_max_worker_read_batches_total: u64,
    usage_runtime_max_worker_read_entries_total: u64,
    usage_runtime_max_worker_reclaimed_entries_total: u64,
    usage_runtime_max_worker_acked_entries_total: u64,
    usage_runtime_max_worker_record_concurrency_limit: u64,
    usage_runtime_max_worker_record_concurrency_in_flight: u64,
    usage_runtime_max_worker_record_concurrency_max_in_flight: u64,
    usage_runtime_max_worker_record_concurrency_wait_total: u64,
    usage_runtime_max_worker_record_deferred_total: u64,
    usage_runtime_max_worker_dead_lettered_entries_total: u64,
    usage_runtime_max_worker_process_failures_total: u64,
    usage_runtime_first_worker_process_failures_total: Option<u64>,
    usage_runtime_final_worker_process_failures_total: u64,
    usage_runtime_worker_process_failures_total_delta: u64,
    usage_runtime_max_worker_read_failures_total: u64,
    usage_runtime_first_worker_read_failures_total: Option<u64>,
    usage_runtime_final_worker_read_failures_total: u64,
    usage_runtime_worker_read_failures_total_delta: u64,
    usage_runtime_max_worker_reclaim_failures_total: u64,
    usage_runtime_first_worker_reclaim_failures_total: Option<u64>,
    usage_runtime_final_worker_reclaim_failures_total: u64,
    usage_runtime_worker_reclaim_failures_total_delta: u64,
    usage_queue_max_group_pending: u64,
    usage_queue_final_group_pending: u64,
    usage_queue_max_group_lag: u64,
    usage_queue_final_group_lag: u64,
    usage_queue_max_oldest_pending_idle_ms: u64,
    usage_queue_final_oldest_pending_idle_ms: u64,
    usage_queue_max_dlq_length: u64,
    usage_queue_final_dlq_length: u64,
    usage_queue_health_unavailable_samples: usize,
    usage_counter_outbox_max_pending_rows: u64,
    usage_counter_outbox_final_pending_rows: u64,
    usage_counter_outbox_max_oldest_pending_age_seconds: u64,
    usage_counter_outbox_final_oldest_pending_age_seconds: u64,
    usage_counter_health_unavailable_samples: usize,
    usage_counter_outbox_max_flush_batches_total: u64,
    usage_counter_outbox_max_flush_rows_claimed_total: u64,
    usage_counter_outbox_max_flush_targets_total: u64,
    usage_counter_outbox_max_flush_failed_batches_total: u64,
    usage_counter_outbox_first_flush_failed_batches_total: Option<u64>,
    usage_counter_outbox_final_flush_failed_batches_total: u64,
    usage_counter_outbox_flush_failed_batches_total_delta: u64,
    usage_counter_outbox_max_cleanup_rows_total: u64,
    usage_counter_outbox_max_cleanup_failed_batches_total: u64,
    usage_counter_outbox_first_cleanup_failed_batches_total: Option<u64>,
    usage_counter_outbox_final_cleanup_failed_batches_total: u64,
    usage_counter_outbox_cleanup_failed_batches_total_delta: u64,
    upstream_target_max_rejected_total: u64,
    upstream_target_max_saturated_total: u64,
    gateway_process_max_cpu_usage_basis_points: u64,
    gateway_process_final_cpu_usage_basis_points: u64,
    gateway_process_max_memory_bytes: u64,
    gateway_process_final_memory_bytes: u64,
    gateway_process_max_memory_basis_points: u64,
    gateway_process_final_memory_basis_points: u64,
    gateway_allocator_observability_available: u64,
    gateway_allocator_max_allocated_bytes: u64,
    gateway_allocator_final_allocated_bytes: u64,
    gateway_allocator_max_active_bytes: u64,
    gateway_allocator_final_active_bytes: u64,
    gateway_allocator_max_resident_bytes: u64,
    gateway_allocator_final_resident_bytes: u64,
    gateway_allocator_max_mapped_bytes: u64,
    gateway_allocator_final_mapped_bytes: u64,
    gateway_allocator_max_retained_bytes: u64,
    gateway_allocator_final_retained_bytes: u64,
    gateway_allocator_max_metadata_bytes: u64,
    gateway_allocator_final_metadata_bytes: u64,
    gateway_allocator_max_active_to_allocated_basis_points: u64,
    gateway_allocator_max_resident_to_allocated_basis_points: u64,
    gateway_process_max_threads: u64,
    gateway_process_final_threads: u64,
    gateway_background_tasks_max_active: u64,
    gateway_background_tasks_final_active: u64,
    gateway_background_tasks_max_supervised_total: u64,
    gateway_background_tasks_max_unexpected_exits_total: u64,
    gateway_background_tasks_max_completed_total: u64,
    gateway_background_tasks_max_panicked_total: u64,
    gateway_background_tasks_max_aborted_total: u64,
    gateway_tokio_runtime_observability_available: u64,
    gateway_tokio_runtime_max_workers: u64,
    gateway_tokio_runtime_final_workers: u64,
    gateway_tokio_runtime_max_alive_tasks: u64,
    gateway_tokio_runtime_final_alive_tasks: u64,
    gateway_tokio_runtime_max_global_queue_depth: u64,
    gateway_tokio_runtime_final_global_queue_depth: u64,
    gateway_process_max_open_fds: u64,
    gateway_process_final_open_fds: u64,
    gateway_process_fd_limit: u64,
    gateway_process_max_fd_usage_basis_points: u64,
    gateway_process_final_fd_usage_basis_points: u64,
    gateway_process_max_socket_fds: u64,
    gateway_process_final_socket_fds: u64,
    gateway_network_observability_available: u64,
    gateway_network_interface_count: u64,
    gateway_network_received_bytes_total_final: u64,
    gateway_network_transmitted_bytes_total_final: u64,
    gateway_network_receive_errors_total_final: u64,
    gateway_network_transmit_errors_total_final: u64,
    gateway_network_receive_dropped_total_final: u64,
    gateway_network_transmit_dropped_total_final: u64,
    gateway_tcp_state_observability_available: u64,
    gateway_host_max_tcp_connections: u64,
    gateway_host_final_tcp_connections: u64,
    gateway_host_max_tcp_established_connections: u64,
    gateway_host_final_tcp_established_connections: u64,
    gateway_host_max_tcp_time_wait_connections: u64,
    gateway_host_final_tcp_time_wait_connections: u64,
    gateway_host_max_tcp_close_wait_connections: u64,
    gateway_host_final_tcp_close_wait_connections: u64,
    gateway_process_max_tcp_connections: u64,
    gateway_process_final_tcp_connections: u64,
    gateway_process_max_tcp_established_connections: u64,
    gateway_process_final_tcp_established_connections: u64,
    gateway_process_max_tcp_time_wait_connections: u64,
    gateway_process_final_tcp_time_wait_connections: u64,
    gateway_process_max_tcp_close_wait_connections: u64,
    gateway_process_final_tcp_close_wait_connections: u64,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayDbPoolPressureWindow {
    sample_index: usize,
    db_pool_checked_out: u64,
    db_pool_idle: Option<u64>,
    db_pool_size: u64,
    db_pool_max_connections: u64,
    db_pool_usage_basis_points: u64,
    db_pool_under_maintenance_pressure: u64,
    postgres_active_connections: u64,
    postgres_waiting_connections: u64,
    postgres_lock_waiting_connections: u64,
    postgres_idle_in_transaction_connections: u64,
    postgres_oldest_active_query_age_ms: u64,
    postgres_oldest_transaction_age_ms: u64,
    gateway_requests_in_flight: u64,
    gateway_requests_distributed_in_flight: u64,
    gateway_auth_snapshot_load_in_flight: u64,
    gateway_auth_snapshot_load_high_watermark: u64,
    gateway_candidate_planning_in_flight: u64,
    gateway_candidate_planning_high_watermark: u64,
    request_candidate_queue_depth: u64,
    request_candidate_queue_pending_depth: u64,
    request_candidate_queue_db_write_in_flight: u64,
    request_candidate_queue_db_write_max_in_flight: u64,
    request_candidate_queue_db_write_wait_total: u64,
    request_candidate_queue_sync_fallback_total: u64,
    usage_runtime_worker_record_concurrency_in_flight: u64,
    usage_runtime_worker_record_concurrency_max_in_flight: u64,
    usage_runtime_worker_record_concurrency_wait_total: u64,
    usage_runtime_worker_record_deferred_total: u64,
    usage_queue_group_pending: u64,
    usage_queue_group_lag: u64,
    usage_counter_outbox_pending_rows: u64,
    usage_counter_outbox_oldest_pending_age_seconds: u64,
    usage_counter_outbox_flush_rows_claimed_total: u64,
    usage_counter_outbox_flush_deferred_total: u64,
    usage_counter_outbox_cleanup_deferred_total: u64,
    redis_runtime_nonblocking_command_latency_ms: u64,
    redis_runtime_lane_command_errors_total: u64,
    redis_runtime_lane_command_timeouts_total: u64,
    redis_runtime_connected_clients: u64,
    redis_runtime_blocked_clients: u64,
    gateway_background_tasks_active: u64,
    gateway_tokio_runtime_alive_tasks: u64,
    gateway_tokio_runtime_global_queue_depth: u64,
    upstream_target_gate_in_flight: u64,
    upstream_target_saturated_total: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    active_background_task_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    postgres_activity_groups: Vec<GatewayPostgresActivityGroup>,
}

#[derive(Debug, Clone, Copy)]
struct GatewayDbPoolPressureBasics {
    sample_index: usize,
    db_pool_checked_out: u64,
    db_pool_idle: Option<u64>,
    db_pool_size: u64,
    db_pool_max_connections: u64,
    db_pool_usage_basis_points: u64,
    db_pool_under_maintenance_pressure: u64,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayPostgresActivityGroup {
    rank: u64,
    state: String,
    wait_event_type: String,
    wait_event: String,
    query_prefix: String,
    connections: u64,
    max_query_age_ms: u64,
    max_transaction_age_ms: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct SettleDrainResult {
    completed: bool,
    elapsed: Duration,
}

impl GatewayPressureMetricsSummary {
    fn observe(&mut self, samples: &[PrometheusSample]) {
        self.samples += 1;
        let db_pool_checked_out = metric_max(samples, "database_pool_checked_out_connections");
        self.db_pool_max_checked_out = self.db_pool_max_checked_out.max(db_pool_checked_out);
        let idle = metric_min(samples, "database_pool_idle_connections");
        self.db_pool_min_idle = match (self.db_pool_min_idle, idle) {
            (Some(current), Some(next)) => Some(current.min(next)),
            (None, Some(next)) => Some(next),
            (current, None) => current,
        };
        let db_pool_size = metric_max(samples, "database_pool_size_connections");
        self.db_pool_max_size = self.db_pool_max_size.max(db_pool_size);
        let db_pool_max_connections = metric_max(samples, "database_pool_max_connections");
        self.db_pool_max_connections = self.db_pool_max_connections.max(db_pool_max_connections);
        let db_pool_usage_basis_points = metric_max(samples, "database_pool_usage_basis_points");
        self.db_pool_max_usage_basis_points = self
            .db_pool_max_usage_basis_points
            .max(db_pool_usage_basis_points);
        self.db_pool_max_idle_reserve = self.db_pool_max_idle_reserve.max(metric_max(
            samples,
            "database_pool_idle_reserve_connections",
        ));
        let db_pool_under_maintenance_pressure =
            metric_max(samples, "database_pool_under_maintenance_pressure");
        if db_pool_under_maintenance_pressure > 0 {
            self.db_pool_pressure_samples += 1;
        }
        if db_pool_pressure_window_triggered(
            db_pool_checked_out,
            db_pool_max_connections,
            db_pool_usage_basis_points,
            db_pool_under_maintenance_pressure,
        ) && self.db_pool_pressure_windows.len() < MAX_DB_POOL_PRESSURE_WINDOWS
        {
            self.db_pool_pressure_windows
                .push(GatewayDbPoolPressureWindow::from_samples(
                    samples,
                    GatewayDbPoolPressureBasics {
                        sample_index: self.samples,
                        db_pool_checked_out,
                        db_pool_idle: idle,
                        db_pool_size,
                        db_pool_max_connections,
                        db_pool_usage_basis_points,
                        db_pool_under_maintenance_pressure,
                    },
                ));
        }
        let postgres_observability_available =
            metric_max(samples, "postgres_observability_available");
        self.postgres_observability_available = self
            .postgres_observability_available
            .max(postgres_observability_available);
        if metric_max(samples, "postgres_observability_unavailable") > 0 {
            self.postgres_observability_unavailable_samples += 1;
        }

        let postgres_active = metric_max(samples, "postgres_active_connections");
        self.postgres_max_active_connections =
            self.postgres_max_active_connections.max(postgres_active);
        self.postgres_final_active_connections = postgres_active;

        let postgres_waiting = metric_max(samples, "postgres_waiting_connections");
        self.postgres_max_waiting_connections =
            self.postgres_max_waiting_connections.max(postgres_waiting);
        self.postgres_final_waiting_connections = postgres_waiting;

        let postgres_lock_waiting = metric_max(samples, "postgres_lock_waiting_connections");
        self.postgres_max_lock_waiting_connections = self
            .postgres_max_lock_waiting_connections
            .max(postgres_lock_waiting);
        self.postgres_final_lock_waiting_connections = postgres_lock_waiting;

        let postgres_idle_in_transaction =
            metric_max(samples, "postgres_idle_in_transaction_connections");
        self.postgres_max_idle_in_transaction_connections = self
            .postgres_max_idle_in_transaction_connections
            .max(postgres_idle_in_transaction);
        self.postgres_final_idle_in_transaction_connections = postgres_idle_in_transaction;

        let postgres_oldest_query = metric_max(samples, "postgres_oldest_active_query_age_ms");
        self.postgres_max_oldest_active_query_age_ms = self
            .postgres_max_oldest_active_query_age_ms
            .max(postgres_oldest_query);
        self.postgres_final_oldest_active_query_age_ms = postgres_oldest_query;

        let postgres_oldest_transaction = metric_max(samples, "postgres_oldest_transaction_age_ms");
        self.postgres_max_oldest_transaction_age_ms = self
            .postgres_max_oldest_transaction_age_ms
            .max(postgres_oldest_transaction);
        self.postgres_final_oldest_transaction_age_ms = postgres_oldest_transaction;
        self.postgres_deadlocks_total_final = metric_max(samples, "postgres_deadlocks_total");
        let postgres_cache_hit_rate =
            metric_max(samples, "postgres_block_cache_hit_rate_basis_points");
        self.postgres_max_block_cache_hit_rate_basis_points = self
            .postgres_max_block_cache_hit_rate_basis_points
            .max(postgres_cache_hit_rate);
        self.postgres_final_block_cache_hit_rate_basis_points = postgres_cache_hit_rate;
        self.postgres_final_temp_bytes_total = metric_max(samples, "postgres_temp_bytes_total");
        self.postgres_final_xact_rollback_total =
            metric_max(samples, "postgres_xact_rollback_total");

        let postgres_wal_observability_available =
            metric_max(samples, "postgres_wal_observability_available");
        self.postgres_wal_observability_available = self
            .postgres_wal_observability_available
            .max(postgres_wal_observability_available);
        if metric_max(samples, "postgres_wal_observability_unavailable") > 0 {
            self.postgres_wal_observability_unavailable_samples += 1;
        }
        self.postgres_final_wal_bytes_total = metric_max(samples, "postgres_wal_bytes_total");
        self.postgres_final_wal_write_time_ms_total =
            metric_max(samples, "postgres_wal_write_time_ms_total");
        self.postgres_final_wal_sync_time_ms_total =
            metric_max(samples, "postgres_wal_sync_time_ms_total");

        let postgres_checkpoint_observability_available =
            metric_max(samples, "postgres_checkpoint_observability_available");
        self.postgres_checkpoint_observability_available = self
            .postgres_checkpoint_observability_available
            .max(postgres_checkpoint_observability_available);
        if metric_max(samples, "postgres_checkpoint_observability_unavailable") > 0 {
            self.postgres_checkpoint_observability_unavailable_samples += 1;
        }
        self.postgres_final_checkpoint_write_time_ms_total =
            metric_max(samples, "postgres_checkpoint_write_time_ms_total");
        self.postgres_final_checkpoint_sync_time_ms_total =
            metric_max(samples, "postgres_checkpoint_sync_time_ms_total");
        self.postgres_final_buffers_checkpoint_total =
            metric_max(samples, "postgres_buffers_checkpoint_total");
        self.postgres_final_buffers_backend_total =
            metric_max(samples, "postgres_buffers_backend_total");

        let postgres_statement_observability_available =
            metric_max(samples, "postgres_statement_observability_available");
        self.postgres_statement_observability_available = self
            .postgres_statement_observability_available
            .max(postgres_statement_observability_available);
        if metric_max(samples, "postgres_statement_observability_unavailable") > 0 {
            self.postgres_statement_observability_unavailable_samples += 1;
        }
        self.postgres_max_statement_top_max_mean_exec_time_ms = self
            .postgres_max_statement_top_max_mean_exec_time_ms
            .max(metric_max(
                samples,
                "postgres_statement_top_max_mean_exec_time_ms",
            ));
        self.postgres_max_statement_top_max_exec_time_ms = self
            .postgres_max_statement_top_max_exec_time_ms
            .max(metric_max(
                samples,
                "postgres_statement_top_max_exec_time_ms",
            ));
        let postgres_statement_top_max_exec_time_ms =
            metric_max(samples, "postgres_statement_top_max_exec_time_ms");
        if postgres_statement_observability_available > 0
            && self.postgres_first_statement_top_max_exec_time_ms.is_none()
        {
            self.postgres_first_statement_top_max_exec_time_ms =
                Some(postgres_statement_top_max_exec_time_ms);
        }
        self.postgres_final_statement_top_max_exec_time_ms =
            postgres_statement_top_max_exec_time_ms;
        self.postgres_statement_top_max_exec_time_ms_delta = delta_from_first(
            self.postgres_first_statement_top_max_exec_time_ms,
            postgres_statement_top_max_exec_time_ms,
        );
        self.postgres_final_statement_top_exec_time_ms_total =
            metric_max(samples, "postgres_statement_top_exec_time_ms_total");
        self.postgres_final_statement_top_temp_blks_total =
            metric_max(samples, "postgres_statement_top_temp_blks_total");

        let redis_runtime_enabled = metric_max(samples, "redis_runtime_enabled");
        let redis_runtime_health_unavailable =
            metric_max(samples, "redis_runtime_health_unavailable");
        let redis_runtime_observability_available =
            redis_runtime_enabled > 0 && redis_runtime_health_unavailable == 0;
        self.redis_runtime_enabled = self.redis_runtime_enabled.max(redis_runtime_enabled);
        if metric_max(samples, "redis_runtime_health_unavailable") > 0 {
            self.redis_runtime_health_unavailable_samples += 1;
        }

        let redis_connected = metric_max(samples, "redis_runtime_connected_clients");
        self.redis_runtime_max_connected_clients = self
            .redis_runtime_max_connected_clients
            .max(redis_connected);
        self.redis_runtime_final_connected_clients = redis_connected;

        let redis_blocked = metric_max(samples, "redis_runtime_blocked_clients");
        self.redis_runtime_max_blocked_clients =
            self.redis_runtime_max_blocked_clients.max(redis_blocked);
        self.redis_runtime_final_blocked_clients = redis_blocked;

        let redis_used_memory = metric_max(samples, "redis_runtime_used_memory_bytes");
        self.redis_runtime_max_used_memory_bytes = self
            .redis_runtime_max_used_memory_bytes
            .max(redis_used_memory);
        self.redis_runtime_final_used_memory_bytes = redis_used_memory;

        let redis_memory_usage_bp = metric_max(samples, "redis_runtime_memory_usage_basis_points");
        self.redis_runtime_max_memory_usage_basis_points = self
            .redis_runtime_max_memory_usage_basis_points
            .max(redis_memory_usage_bp);
        self.redis_runtime_final_memory_usage_basis_points = redis_memory_usage_bp;

        self.redis_runtime_max_memory_fragmentation_ratio_basis_points = self
            .redis_runtime_max_memory_fragmentation_ratio_basis_points
            .max(metric_max(
                samples,
                "redis_runtime_memory_fragmentation_ratio_basis_points",
            ));
        self.redis_runtime_max_instantaneous_ops_per_sec = self
            .redis_runtime_max_instantaneous_ops_per_sec
            .max(metric_max(
                samples,
                "redis_runtime_instantaneous_ops_per_sec",
            ));
        self.redis_runtime_max_rejected_connections_total = self
            .redis_runtime_max_rejected_connections_total
            .max(metric_max(
                samples,
                "redis_runtime_rejected_connections_total",
            ));
        let redis_rejected_connections_total =
            metric_max(samples, "redis_runtime_rejected_connections_total");
        if redis_runtime_observability_available
            && self
                .redis_runtime_first_rejected_connections_total
                .is_none()
        {
            self.redis_runtime_first_rejected_connections_total =
                Some(redis_rejected_connections_total);
        }
        self.redis_runtime_final_rejected_connections_total = redis_rejected_connections_total;
        self.redis_runtime_rejected_connections_total_delta = delta_from_first(
            self.redis_runtime_first_rejected_connections_total,
            redis_rejected_connections_total,
        );
        self.redis_runtime_max_evicted_keys_total = self
            .redis_runtime_max_evicted_keys_total
            .max(metric_max(samples, "redis_runtime_evicted_keys_total"));
        let redis_evicted_keys_total = metric_max(samples, "redis_runtime_evicted_keys_total");
        if redis_runtime_observability_available
            && self.redis_runtime_first_evicted_keys_total.is_none()
        {
            self.redis_runtime_first_evicted_keys_total = Some(redis_evicted_keys_total);
        }
        self.redis_runtime_final_evicted_keys_total = redis_evicted_keys_total;
        self.redis_runtime_evicted_keys_total_delta = delta_from_first(
            self.redis_runtime_first_evicted_keys_total,
            redis_evicted_keys_total,
        );
        self.redis_runtime_max_total_error_replies = self
            .redis_runtime_max_total_error_replies
            .max(metric_max(samples, "redis_runtime_total_error_replies"));
        let redis_total_error_replies = metric_max(samples, "redis_runtime_total_error_replies");
        if redis_runtime_observability_available
            && self.redis_runtime_first_total_error_replies.is_none()
        {
            self.redis_runtime_first_total_error_replies = Some(redis_total_error_replies);
        }
        self.redis_runtime_final_total_error_replies = redis_total_error_replies;
        self.redis_runtime_total_error_replies_delta = delta_from_first(
            self.redis_runtime_first_total_error_replies,
            redis_total_error_replies,
        );
        self.redis_runtime_max_lane_command_errors_total = self
            .redis_runtime_max_lane_command_errors_total
            .max(metric_sum(
                samples,
                "redis_runtime_lane_command_errors_total",
            ));
        let redis_lane_command_errors_total =
            metric_sum(samples, "redis_runtime_lane_command_errors_total");
        if redis_runtime_observability_available
            && self.redis_runtime_first_lane_command_errors_total.is_none()
        {
            self.redis_runtime_first_lane_command_errors_total =
                Some(redis_lane_command_errors_total);
        }
        self.redis_runtime_final_lane_command_errors_total = redis_lane_command_errors_total;
        self.redis_runtime_lane_command_errors_total_delta = delta_from_first(
            self.redis_runtime_first_lane_command_errors_total,
            redis_lane_command_errors_total,
        );
        self.redis_runtime_max_lane_command_timeouts_total = self
            .redis_runtime_max_lane_command_timeouts_total
            .max(metric_sum(
                samples,
                "redis_runtime_lane_command_timeouts_total",
            ));
        let redis_lane_command_timeouts_total =
            metric_sum(samples, "redis_runtime_lane_command_timeouts_total");
        if redis_runtime_observability_available
            && self
                .redis_runtime_first_lane_command_timeouts_total
                .is_none()
        {
            self.redis_runtime_first_lane_command_timeouts_total =
                Some(redis_lane_command_timeouts_total);
        }
        self.redis_runtime_final_lane_command_timeouts_total = redis_lane_command_timeouts_total;
        self.redis_runtime_lane_command_timeouts_total_delta = delta_from_first(
            self.redis_runtime_first_lane_command_timeouts_total,
            redis_lane_command_timeouts_total,
        );
        self.redis_runtime_max_command_count_total =
            self.redis_runtime_max_command_count_total.max(metric_sum(
                samples,
                "redis_runtime_lane_command_count_total",
            ));

        let redis_fast_latency =
            metric_for_lane(samples, "redis_runtime_lane_command_latency_ms_max", "fast");
        self.redis_runtime_max_fast_command_latency_ms = self
            .redis_runtime_max_fast_command_latency_ms
            .max(redis_fast_latency);
        let redis_stream_latency = metric_for_lane(
            samples,
            "redis_runtime_lane_command_latency_ms_max",
            "stream",
        );
        self.redis_runtime_max_stream_command_latency_ms = self
            .redis_runtime_max_stream_command_latency_ms
            .max(redis_stream_latency);
        let redis_admin_latency = metric_for_lane(
            samples,
            "redis_runtime_lane_command_latency_ms_max",
            "admin",
        );
        self.redis_runtime_max_admin_command_latency_ms = self
            .redis_runtime_max_admin_command_latency_ms
            .max(redis_admin_latency);
        let redis_blocking_stream_latency = metric_for_lane(
            samples,
            "redis_runtime_lane_command_latency_ms_max",
            "blocking_stream",
        );
        self.redis_runtime_max_blocking_stream_command_latency_ms = self
            .redis_runtime_max_blocking_stream_command_latency_ms
            .max(redis_blocking_stream_latency);
        let redis_nonblocking_latency = redis_fast_latency
            .max(redis_stream_latency)
            .max(redis_admin_latency);
        self.redis_runtime_max_nonblocking_command_latency_ms = self
            .redis_runtime_max_nonblocking_command_latency_ms
            .max(redis_nonblocking_latency);
        if redis_runtime_observability_available
            && self
                .redis_runtime_first_nonblocking_command_latency_ms
                .is_none()
        {
            self.redis_runtime_first_nonblocking_command_latency_ms =
                Some(redis_nonblocking_latency);
        }
        self.redis_runtime_final_nonblocking_command_latency_ms = redis_nonblocking_latency;
        self.redis_runtime_nonblocking_command_latency_ms_delta = delta_from_first(
            self.redis_runtime_first_nonblocking_command_latency_ms,
            self.redis_runtime_max_nonblocking_command_latency_ms,
        );
        let redis_nonblocking_command_count_total = ["fast", "stream", "admin"]
            .into_iter()
            .map(|lane| {
                metric_for_lane_and_bucket(
                    samples,
                    "redis_runtime_lane_command_latency_ms_bucket",
                    lane,
                    "+Inf",
                )
            })
            .sum();
        let redis_nonblocking_command_le_500ms_total = ["fast", "stream", "admin"]
            .into_iter()
            .map(|lane| {
                metric_for_lane_and_bucket(
                    samples,
                    "redis_runtime_lane_command_latency_ms_bucket",
                    lane,
                    "500",
                )
            })
            .sum();
        if redis_runtime_observability_available
            && self
                .redis_runtime_first_nonblocking_command_count_total
                .is_none()
        {
            self.redis_runtime_first_nonblocking_command_count_total =
                Some(redis_nonblocking_command_count_total);
            self.redis_runtime_first_nonblocking_command_le_500ms_total =
                Some(redis_nonblocking_command_le_500ms_total);
        }
        self.redis_runtime_final_nonblocking_command_count_total =
            redis_nonblocking_command_count_total;
        self.redis_runtime_final_nonblocking_command_le_500ms_total =
            redis_nonblocking_command_le_500ms_total;
        self.redis_runtime_nonblocking_command_count_total_delta = delta_from_first(
            self.redis_runtime_first_nonblocking_command_count_total,
            redis_nonblocking_command_count_total,
        );
        self.redis_runtime_nonblocking_command_le_500ms_total_delta = delta_from_first(
            self.redis_runtime_first_nonblocking_command_le_500ms_total,
            redis_nonblocking_command_le_500ms_total,
        );
        self.redis_runtime_nonblocking_command_over_500ms_total_delta = self
            .redis_runtime_nonblocking_command_count_total_delta
            .saturating_sub(self.redis_runtime_nonblocking_command_le_500ms_total_delta);
        self.redis_runtime_nonblocking_command_over_500ms_rate_basis_points = self
            .redis_runtime_nonblocking_command_over_500ms_total_delta
            .saturating_mul(10_000)
            .checked_div(self.redis_runtime_nonblocking_command_count_total_delta)
            .unwrap_or_default();
        self.redis_runtime_max_command_latency_ms = self
            .redis_runtime_max_command_latency_ms
            .max(redis_nonblocking_latency.max(redis_blocking_stream_latency));

        self.gateway_requests_max_in_flight = self.gateway_requests_max_in_flight.max(
            find_metric_value_u64(
                samples,
                "concurrency_in_flight",
                &[("gate", "gateway_requests")],
            )
            .unwrap_or_default(),
        );
        self.gateway_requests_max_rejected_total = self.gateway_requests_max_rejected_total.max(
            find_metric_value_u64(
                samples,
                "concurrency_rejected_total",
                &[("gate", "gateway_requests")],
            )
            .unwrap_or_default(),
        );
        self.gateway_requests_distributed_max_in_flight =
            self.gateway_requests_distributed_max_in_flight.max(
                find_metric_value_u64(
                    samples,
                    "concurrency_in_flight",
                    &[("gate", "gateway_requests_distributed")],
                )
                .unwrap_or_default(),
            );
        self.gateway_requests_distributed_max_rejected_total =
            self.gateway_requests_distributed_max_rejected_total.max(
                find_metric_value_u64(
                    samples,
                    "concurrency_rejected_total",
                    &[("gate", "gateway_requests_distributed")],
                )
                .unwrap_or_default(),
            );
        let candidate_depth = metric_max(samples, "request_candidate_queue_depth");
        self.request_candidate_queue_max_depth =
            self.request_candidate_queue_max_depth.max(candidate_depth);
        self.request_candidate_queue_final_depth = candidate_depth;

        let candidate_pending_depth = metric_max(samples, "request_candidate_queue_pending_depth");
        self.request_candidate_queue_max_pending_depth = self
            .request_candidate_queue_max_pending_depth
            .max(candidate_pending_depth);
        self.request_candidate_queue_final_pending_depth = candidate_pending_depth;

        self.request_candidate_queue_capacity = self
            .request_candidate_queue_capacity
            .max(metric_max(samples, "request_candidate_queue_capacity"));
        self.request_candidate_queue_max_enqueued_total = self
            .request_candidate_queue_max_enqueued_total
            .max(metric_max(
                samples,
                "request_candidate_queue_enqueued_total",
            ));
        self.request_candidate_queue_max_flushed_total = self
            .request_candidate_queue_max_flushed_total
            .max(metric_max(samples, "request_candidate_queue_flushed_total"));
        self.request_candidate_queue_max_flush_batches_total = self
            .request_candidate_queue_max_flush_batches_total
            .max(metric_max(
                samples,
                "request_candidate_queue_flush_batches_total",
            ));
        self.request_candidate_queue_max_flush_sql_ops_total = self
            .request_candidate_queue_max_flush_sql_ops_total
            .max(metric_max(
                samples,
                "request_candidate_queue_flush_sql_ops_total",
            ));
        self.request_candidate_queue_max_flush_sql_records_total = self
            .request_candidate_queue_max_flush_sql_records_total
            .max(metric_max(
                samples,
                "request_candidate_queue_flush_sql_records_total",
            ));
        self.request_candidate_queue_max_db_write_concurrency_limit = self
            .request_candidate_queue_max_db_write_concurrency_limit
            .max(metric_max(
                samples,
                "request_candidate_queue_db_write_concurrency_limit",
            ));
        self.request_candidate_queue_max_db_write_max_in_flight = self
            .request_candidate_queue_max_db_write_max_in_flight
            .max(metric_max(
                samples,
                "request_candidate_queue_db_write_max_in_flight",
            ));
        self.request_candidate_queue_max_db_write_wait_total = self
            .request_candidate_queue_max_db_write_wait_total
            .max(metric_max(
                samples,
                "request_candidate_queue_db_write_wait_total",
            ));
        self.request_candidate_queue_max_compacted_total = self
            .request_candidate_queue_max_compacted_total
            .max(metric_max(
                samples,
                "request_candidate_queue_compacted_total",
            ));
        self.request_candidate_queue_max_dropped_total = self
            .request_candidate_queue_max_dropped_total
            .max(metric_max(samples, "request_candidate_queue_dropped_total"));
        self.request_candidate_queue_max_flush_failed_total = self
            .request_candidate_queue_max_flush_failed_total
            .max(metric_max(
                samples,
                "request_candidate_queue_flush_failed_total",
            ));
        self.request_candidate_queue_max_sync_fallback_total = self
            .request_candidate_queue_max_sync_fallback_total
            .max(metric_max(
                samples,
                "request_candidate_queue_sync_fallback_total",
            ));
        self.usage_runtime_max_terminal_enqueue_failed_total = self
            .usage_runtime_max_terminal_enqueue_failed_total
            .max(metric_max(
                samples,
                "usage_runtime_terminal_enqueue_failed_total",
            ));
        self.usage_runtime_max_lifecycle_enqueue_failed_total = self
            .usage_runtime_max_lifecycle_enqueue_failed_total
            .max(metric_max(
                samples,
                "usage_runtime_lifecycle_enqueue_failed_total",
            ));
        self.usage_runtime_max_lifecycle_enqueue_deferred_dropped_total = self
            .usage_runtime_max_lifecycle_enqueue_deferred_dropped_total
            .max(metric_max(
                samples,
                "usage_runtime_lifecycle_enqueue_deferred_dropped_total",
            ));
        let enqueue_retry_pending = metric_max(samples, "usage_runtime_enqueue_retry_pending");
        self.usage_runtime_max_enqueue_retry_pending = self
            .usage_runtime_max_enqueue_retry_pending
            .max(enqueue_retry_pending);
        self.usage_runtime_final_enqueue_retry_pending = enqueue_retry_pending;

        let enqueue_retry_scheduled_total =
            metric_max(samples, "usage_runtime_enqueue_retry_scheduled_total");
        if self
            .usage_runtime_first_enqueue_retry_scheduled_total
            .is_none()
        {
            self.usage_runtime_first_enqueue_retry_scheduled_total =
                Some(enqueue_retry_scheduled_total);
        }
        self.usage_runtime_final_enqueue_retry_scheduled_total = enqueue_retry_scheduled_total;
        self.usage_runtime_enqueue_retry_scheduled_total_delta = delta_from_first(
            self.usage_runtime_first_enqueue_retry_scheduled_total,
            enqueue_retry_scheduled_total,
        );

        let enqueue_retry_recovered_total =
            metric_max(samples, "usage_runtime_enqueue_retry_recovered_total");
        if self
            .usage_runtime_first_enqueue_retry_recovered_total
            .is_none()
        {
            self.usage_runtime_first_enqueue_retry_recovered_total =
                Some(enqueue_retry_recovered_total);
        }
        self.usage_runtime_final_enqueue_retry_recovered_total = enqueue_retry_recovered_total;
        self.usage_runtime_enqueue_retry_recovered_total_delta = delta_from_first(
            self.usage_runtime_first_enqueue_retry_recovered_total,
            enqueue_retry_recovered_total,
        );

        let enqueue_retry_failed_total =
            metric_max(samples, "usage_runtime_enqueue_retry_failed_total");
        if self
            .usage_runtime_first_enqueue_retry_failed_total
            .is_none()
        {
            self.usage_runtime_first_enqueue_retry_failed_total = Some(enqueue_retry_failed_total);
        }
        self.usage_runtime_final_enqueue_retry_failed_total = enqueue_retry_failed_total;
        self.usage_runtime_enqueue_retry_failed_total_delta = delta_from_first(
            self.usage_runtime_first_enqueue_retry_failed_total,
            enqueue_retry_failed_total,
        );

        let enqueue_retry_closed_or_unavailable_total = metric_max(
            samples,
            "usage_runtime_enqueue_retry_closed_or_unavailable_total",
        );
        if self
            .usage_runtime_first_enqueue_retry_closed_or_unavailable_total
            .is_none()
        {
            self.usage_runtime_first_enqueue_retry_closed_or_unavailable_total =
                Some(enqueue_retry_closed_or_unavailable_total);
        }
        self.usage_runtime_final_enqueue_retry_closed_or_unavailable_total =
            enqueue_retry_closed_or_unavailable_total;
        self.usage_runtime_enqueue_retry_closed_or_unavailable_total_delta = delta_from_first(
            self.usage_runtime_first_enqueue_retry_closed_or_unavailable_total,
            enqueue_retry_closed_or_unavailable_total,
        );
        self.usage_runtime_max_worker_read_batches_total = self
            .usage_runtime_max_worker_read_batches_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_read_batches_total",
            ));
        self.usage_runtime_max_worker_read_entries_total = self
            .usage_runtime_max_worker_read_entries_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_read_entries_total",
            ));
        self.usage_runtime_max_worker_reclaimed_entries_total = self
            .usage_runtime_max_worker_reclaimed_entries_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_reclaimed_entries_total",
            ));
        self.usage_runtime_max_worker_acked_entries_total = self
            .usage_runtime_max_worker_acked_entries_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_acked_entries_total",
            ));
        self.usage_runtime_max_worker_record_concurrency_limit = self
            .usage_runtime_max_worker_record_concurrency_limit
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_record_concurrency_limit",
            ));
        self.usage_runtime_max_worker_record_concurrency_in_flight = self
            .usage_runtime_max_worker_record_concurrency_in_flight
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_record_concurrency_in_flight",
            ));
        self.usage_runtime_max_worker_record_concurrency_max_in_flight = self
            .usage_runtime_max_worker_record_concurrency_max_in_flight
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_record_concurrency_max_in_flight",
            ));
        self.usage_runtime_max_worker_record_concurrency_wait_total = self
            .usage_runtime_max_worker_record_concurrency_wait_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_record_concurrency_wait_total",
            ));
        self.usage_runtime_max_worker_record_deferred_total = self
            .usage_runtime_max_worker_record_deferred_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_record_deferred_total",
            ));
        self.usage_runtime_max_worker_dead_lettered_entries_total = self
            .usage_runtime_max_worker_dead_lettered_entries_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_dead_lettered_entries_total",
            ));
        self.usage_runtime_max_worker_process_failures_total = self
            .usage_runtime_max_worker_process_failures_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_process_failures_total",
            ));
        let worker_process_failures =
            metric_max(samples, "usage_runtime_queue_worker_process_failures_total");
        if self
            .usage_runtime_first_worker_process_failures_total
            .is_none()
        {
            self.usage_runtime_first_worker_process_failures_total = Some(worker_process_failures);
        }
        self.usage_runtime_final_worker_process_failures_total = worker_process_failures;
        self.usage_runtime_worker_process_failures_total_delta = delta_from_first(
            self.usage_runtime_first_worker_process_failures_total,
            worker_process_failures,
        );
        self.usage_runtime_max_worker_read_failures_total = self
            .usage_runtime_max_worker_read_failures_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_read_failures_total",
            ));
        let worker_read_failures =
            metric_max(samples, "usage_runtime_queue_worker_read_failures_total");
        if self
            .usage_runtime_first_worker_read_failures_total
            .is_none()
        {
            self.usage_runtime_first_worker_read_failures_total = Some(worker_read_failures);
        }
        self.usage_runtime_final_worker_read_failures_total = worker_read_failures;
        self.usage_runtime_worker_read_failures_total_delta = delta_from_first(
            self.usage_runtime_first_worker_read_failures_total,
            worker_read_failures,
        );
        self.usage_runtime_max_worker_reclaim_failures_total = self
            .usage_runtime_max_worker_reclaim_failures_total
            .max(metric_max(
                samples,
                "usage_runtime_queue_worker_reclaim_failures_total",
            ));
        let worker_reclaim_failures =
            metric_max(samples, "usage_runtime_queue_worker_reclaim_failures_total");
        if self
            .usage_runtime_first_worker_reclaim_failures_total
            .is_none()
        {
            self.usage_runtime_first_worker_reclaim_failures_total = Some(worker_reclaim_failures);
        }
        self.usage_runtime_final_worker_reclaim_failures_total = worker_reclaim_failures;
        self.usage_runtime_worker_reclaim_failures_total_delta = delta_from_first(
            self.usage_runtime_first_worker_reclaim_failures_total,
            worker_reclaim_failures,
        );
        let usage_queue_pending = metric_max(samples, "usage_queue_group_pending");
        self.usage_queue_max_group_pending =
            self.usage_queue_max_group_pending.max(usage_queue_pending);
        self.usage_queue_final_group_pending = usage_queue_pending;

        let usage_queue_lag = metric_max(samples, "usage_queue_group_lag");
        self.usage_queue_max_group_lag = self.usage_queue_max_group_lag.max(usage_queue_lag);
        self.usage_queue_final_group_lag = usage_queue_lag;

        let usage_queue_oldest_idle = metric_max(samples, "usage_queue_oldest_pending_idle_ms");
        self.usage_queue_max_oldest_pending_idle_ms = self
            .usage_queue_max_oldest_pending_idle_ms
            .max(usage_queue_oldest_idle);
        self.usage_queue_final_oldest_pending_idle_ms = usage_queue_oldest_idle;

        let usage_queue_dlq_length = metric_max(samples, "usage_queue_dlq_length");
        self.usage_queue_max_dlq_length =
            self.usage_queue_max_dlq_length.max(usage_queue_dlq_length);
        self.usage_queue_final_dlq_length = usage_queue_dlq_length;
        if metric_max(samples, "usage_queue_health_unavailable") > 0 {
            self.usage_queue_health_unavailable_samples += 1;
        }
        let usage_counter_pending = metric_max(samples, "usage_counter_outbox_pending_rows");
        self.usage_counter_outbox_max_pending_rows = self
            .usage_counter_outbox_max_pending_rows
            .max(usage_counter_pending);
        self.usage_counter_outbox_final_pending_rows = usage_counter_pending;

        let usage_counter_oldest_age =
            metric_max(samples, "usage_counter_outbox_oldest_pending_age_seconds");
        self.usage_counter_outbox_max_oldest_pending_age_seconds = self
            .usage_counter_outbox_max_oldest_pending_age_seconds
            .max(usage_counter_oldest_age);
        self.usage_counter_outbox_final_oldest_pending_age_seconds = usage_counter_oldest_age;
        if metric_max(samples, "usage_counter_health_unavailable") > 0 {
            self.usage_counter_health_unavailable_samples += 1;
        }
        self.usage_counter_outbox_max_flush_batches_total = self
            .usage_counter_outbox_max_flush_batches_total
            .max(metric_max(
                samples,
                "usage_counter_outbox_flush_batches_total",
            ));
        self.usage_counter_outbox_max_flush_rows_claimed_total = self
            .usage_counter_outbox_max_flush_rows_claimed_total
            .max(metric_max(
                samples,
                "usage_counter_outbox_flush_rows_claimed_total",
            ));
        self.usage_counter_outbox_max_flush_targets_total = self
            .usage_counter_outbox_max_flush_targets_total
            .max(metric_sum(
                samples,
                "usage_counter_outbox_flush_targets_total",
            ));
        self.usage_counter_outbox_max_flush_failed_batches_total = self
            .usage_counter_outbox_max_flush_failed_batches_total
            .max(metric_max(
                samples,
                "usage_counter_outbox_flush_failed_batches_total",
            ));
        let usage_counter_flush_failed =
            metric_max(samples, "usage_counter_outbox_flush_failed_batches_total");
        if self
            .usage_counter_outbox_first_flush_failed_batches_total
            .is_none()
        {
            self.usage_counter_outbox_first_flush_failed_batches_total =
                Some(usage_counter_flush_failed);
        }
        self.usage_counter_outbox_final_flush_failed_batches_total = usage_counter_flush_failed;
        self.usage_counter_outbox_flush_failed_batches_total_delta = delta_from_first(
            self.usage_counter_outbox_first_flush_failed_batches_total,
            usage_counter_flush_failed,
        );
        self.usage_counter_outbox_max_cleanup_rows_total = self
            .usage_counter_outbox_max_cleanup_rows_total
            .max(metric_max(
                samples,
                "usage_counter_outbox_cleanup_rows_total",
            ));
        self.usage_counter_outbox_max_cleanup_failed_batches_total = self
            .usage_counter_outbox_max_cleanup_failed_batches_total
            .max(metric_max(
                samples,
                "usage_counter_outbox_cleanup_failed_batches_total",
            ));
        let usage_counter_cleanup_failed =
            metric_max(samples, "usage_counter_outbox_cleanup_failed_batches_total");
        if self
            .usage_counter_outbox_first_cleanup_failed_batches_total
            .is_none()
        {
            self.usage_counter_outbox_first_cleanup_failed_batches_total =
                Some(usage_counter_cleanup_failed);
        }
        self.usage_counter_outbox_final_cleanup_failed_batches_total = usage_counter_cleanup_failed;
        self.usage_counter_outbox_cleanup_failed_batches_total_delta = delta_from_first(
            self.usage_counter_outbox_first_cleanup_failed_batches_total,
            usage_counter_cleanup_failed,
        );
        self.upstream_target_max_rejected_total = self
            .upstream_target_max_rejected_total
            .max(metric_sum(samples, "upstream_target_gate_rejected_total"));
        self.upstream_target_max_saturated_total = self
            .upstream_target_max_saturated_total
            .max(metric_sum(samples, "upstream_target_saturated_total"));
        let process_cpu = metric_max(samples, "gateway_process_cpu_usage_basis_points");
        self.gateway_process_max_cpu_usage_basis_points = self
            .gateway_process_max_cpu_usage_basis_points
            .max(process_cpu);
        self.gateway_process_final_cpu_usage_basis_points = process_cpu;

        let process_memory = metric_max(samples, "gateway_process_memory_bytes");
        self.gateway_process_max_memory_bytes =
            self.gateway_process_max_memory_bytes.max(process_memory);
        self.gateway_process_final_memory_bytes = process_memory;

        let process_memory_bp = metric_max(samples, "gateway_process_memory_basis_points");
        self.gateway_process_max_memory_basis_points = self
            .gateway_process_max_memory_basis_points
            .max(process_memory_bp);
        self.gateway_process_final_memory_basis_points = process_memory_bp;

        self.gateway_allocator_observability_available = self
            .gateway_allocator_observability_available
            .max(metric_max(
                samples,
                "gateway_allocator_observability_available",
            ));
        let allocator_allocated = metric_max(samples, "gateway_allocator_allocated_bytes");
        self.gateway_allocator_max_allocated_bytes = self
            .gateway_allocator_max_allocated_bytes
            .max(allocator_allocated);
        self.gateway_allocator_final_allocated_bytes = allocator_allocated;

        let allocator_active = metric_max(samples, "gateway_allocator_active_bytes");
        self.gateway_allocator_max_active_bytes = self
            .gateway_allocator_max_active_bytes
            .max(allocator_active);
        self.gateway_allocator_final_active_bytes = allocator_active;

        let allocator_resident = metric_max(samples, "gateway_allocator_resident_bytes");
        self.gateway_allocator_max_resident_bytes = self
            .gateway_allocator_max_resident_bytes
            .max(allocator_resident);
        self.gateway_allocator_final_resident_bytes = allocator_resident;

        let allocator_mapped = metric_max(samples, "gateway_allocator_mapped_bytes");
        self.gateway_allocator_max_mapped_bytes = self
            .gateway_allocator_max_mapped_bytes
            .max(allocator_mapped);
        self.gateway_allocator_final_mapped_bytes = allocator_mapped;

        let allocator_retained = metric_max(samples, "gateway_allocator_retained_bytes");
        self.gateway_allocator_max_retained_bytes = self
            .gateway_allocator_max_retained_bytes
            .max(allocator_retained);
        self.gateway_allocator_final_retained_bytes = allocator_retained;

        let allocator_metadata = metric_max(samples, "gateway_allocator_metadata_bytes");
        self.gateway_allocator_max_metadata_bytes = self
            .gateway_allocator_max_metadata_bytes
            .max(allocator_metadata);
        self.gateway_allocator_final_metadata_bytes = allocator_metadata;

        self.gateway_allocator_max_active_to_allocated_basis_points = self
            .gateway_allocator_max_active_to_allocated_basis_points
            .max(metric_max(
                samples,
                "gateway_allocator_active_to_allocated_basis_points",
            ));
        self.gateway_allocator_max_resident_to_allocated_basis_points = self
            .gateway_allocator_max_resident_to_allocated_basis_points
            .max(metric_max(
                samples,
                "gateway_allocator_resident_to_allocated_basis_points",
            ));

        let process_threads = metric_max(samples, "gateway_process_threads");
        self.gateway_process_max_threads = self.gateway_process_max_threads.max(process_threads);
        self.gateway_process_final_threads = process_threads;

        let background_tasks_active = metric_max(samples, "gateway_background_tasks_active");
        self.gateway_background_tasks_max_active = self
            .gateway_background_tasks_max_active
            .max(background_tasks_active);
        self.gateway_background_tasks_final_active = background_tasks_active;
        self.gateway_background_tasks_max_supervised_total = self
            .gateway_background_tasks_max_supervised_total
            .max(metric_max(
                samples,
                "gateway_background_tasks_supervised_total",
            ));
        self.gateway_background_tasks_max_unexpected_exits_total = self
            .gateway_background_tasks_max_unexpected_exits_total
            .max(metric_max(
                samples,
                "gateway_background_tasks_unexpected_exits_total",
            ));
        self.gateway_background_tasks_max_completed_total = self
            .gateway_background_tasks_max_completed_total
            .max(metric_max(
                samples,
                "gateway_background_tasks_completed_total",
            ));
        self.gateway_background_tasks_max_panicked_total = self
            .gateway_background_tasks_max_panicked_total
            .max(metric_max(
                samples,
                "gateway_background_tasks_panicked_total",
            ));
        self.gateway_background_tasks_max_aborted_total = self
            .gateway_background_tasks_max_aborted_total
            .max(metric_max(
                samples,
                "gateway_background_tasks_aborted_total",
            ));

        self.gateway_tokio_runtime_observability_available = self
            .gateway_tokio_runtime_observability_available
            .max(metric_max(
                samples,
                "gateway_tokio_runtime_observability_available",
            ));
        let tokio_workers = metric_max(samples, "gateway_tokio_runtime_workers");
        self.gateway_tokio_runtime_max_workers =
            self.gateway_tokio_runtime_max_workers.max(tokio_workers);
        self.gateway_tokio_runtime_final_workers = tokio_workers;

        let tokio_alive_tasks = metric_max(samples, "gateway_tokio_runtime_alive_tasks");
        self.gateway_tokio_runtime_max_alive_tasks = self
            .gateway_tokio_runtime_max_alive_tasks
            .max(tokio_alive_tasks);
        self.gateway_tokio_runtime_final_alive_tasks = tokio_alive_tasks;

        let tokio_global_queue_depth =
            metric_max(samples, "gateway_tokio_runtime_global_queue_depth");
        self.gateway_tokio_runtime_max_global_queue_depth = self
            .gateway_tokio_runtime_max_global_queue_depth
            .max(tokio_global_queue_depth);
        self.gateway_tokio_runtime_final_global_queue_depth = tokio_global_queue_depth;

        let process_open_fds = metric_max(samples, "gateway_process_open_fds");
        self.gateway_process_max_open_fds = self.gateway_process_max_open_fds.max(process_open_fds);
        self.gateway_process_final_open_fds = process_open_fds;
        self.gateway_process_fd_limit = self
            .gateway_process_fd_limit
            .max(metric_max(samples, "gateway_process_fd_limit"));

        let process_fd_usage_bp = metric_max(samples, "gateway_process_fd_usage_basis_points");
        self.gateway_process_max_fd_usage_basis_points = self
            .gateway_process_max_fd_usage_basis_points
            .max(process_fd_usage_bp);
        self.gateway_process_final_fd_usage_basis_points = process_fd_usage_bp;

        let process_socket_fds = metric_max(samples, "gateway_process_socket_fds");
        self.gateway_process_max_socket_fds =
            self.gateway_process_max_socket_fds.max(process_socket_fds);
        self.gateway_process_final_socket_fds = process_socket_fds;

        self.gateway_network_observability_available =
            self.gateway_network_observability_available.max(metric_max(
                samples,
                "gateway_network_observability_available",
            ));
        self.gateway_network_interface_count = self
            .gateway_network_interface_count
            .max(metric_max(samples, "gateway_network_interfaces"));
        self.gateway_network_received_bytes_total_final =
            metric_max(samples, "gateway_network_received_bytes_total");
        self.gateway_network_transmitted_bytes_total_final =
            metric_max(samples, "gateway_network_transmitted_bytes_total");
        self.gateway_network_receive_errors_total_final =
            metric_max(samples, "gateway_network_receive_errors_total");
        self.gateway_network_transmit_errors_total_final =
            metric_max(samples, "gateway_network_transmit_errors_total");
        self.gateway_network_receive_dropped_total_final =
            metric_max(samples, "gateway_network_receive_dropped_total");
        self.gateway_network_transmit_dropped_total_final =
            metric_max(samples, "gateway_network_transmit_dropped_total");

        self.gateway_tcp_state_observability_available = self
            .gateway_tcp_state_observability_available
            .max(metric_max(
                samples,
                "gateway_tcp_state_observability_available",
            ));

        let host_tcp_connections = metric_max(samples, "gateway_host_tcp_connections");
        self.gateway_host_max_tcp_connections = self
            .gateway_host_max_tcp_connections
            .max(host_tcp_connections);
        self.gateway_host_final_tcp_connections = host_tcp_connections;

        let host_tcp_established = metric_max(samples, "gateway_host_tcp_established_connections");
        self.gateway_host_max_tcp_established_connections = self
            .gateway_host_max_tcp_established_connections
            .max(host_tcp_established);
        self.gateway_host_final_tcp_established_connections = host_tcp_established;

        let host_tcp_time_wait = metric_max(samples, "gateway_host_tcp_time_wait_connections");
        self.gateway_host_max_tcp_time_wait_connections = self
            .gateway_host_max_tcp_time_wait_connections
            .max(host_tcp_time_wait);
        self.gateway_host_final_tcp_time_wait_connections = host_tcp_time_wait;

        let host_tcp_close_wait = metric_max(samples, "gateway_host_tcp_close_wait_connections");
        self.gateway_host_max_tcp_close_wait_connections = self
            .gateway_host_max_tcp_close_wait_connections
            .max(host_tcp_close_wait);
        self.gateway_host_final_tcp_close_wait_connections = host_tcp_close_wait;

        let process_tcp_connections = metric_max(samples, "gateway_process_tcp_connections");
        self.gateway_process_max_tcp_connections = self
            .gateway_process_max_tcp_connections
            .max(process_tcp_connections);
        self.gateway_process_final_tcp_connections = process_tcp_connections;

        let process_tcp_established =
            metric_max(samples, "gateway_process_tcp_established_connections");
        self.gateway_process_max_tcp_established_connections = self
            .gateway_process_max_tcp_established_connections
            .max(process_tcp_established);
        self.gateway_process_final_tcp_established_connections = process_tcp_established;

        let process_tcp_time_wait =
            metric_max(samples, "gateway_process_tcp_time_wait_connections");
        self.gateway_process_max_tcp_time_wait_connections = self
            .gateway_process_max_tcp_time_wait_connections
            .max(process_tcp_time_wait);
        self.gateway_process_final_tcp_time_wait_connections = process_tcp_time_wait;

        let process_tcp_close_wait =
            metric_max(samples, "gateway_process_tcp_close_wait_connections");
        self.gateway_process_max_tcp_close_wait_connections = self
            .gateway_process_max_tcp_close_wait_connections
            .max(process_tcp_close_wait);
        self.gateway_process_final_tcp_close_wait_connections = process_tcp_close_wait;
    }
}

impl GatewayDbPoolPressureWindow {
    fn from_samples(samples: &[PrometheusSample], basics: GatewayDbPoolPressureBasics) -> Self {
        Self {
            sample_index: basics.sample_index,
            db_pool_checked_out: basics.db_pool_checked_out,
            db_pool_idle: basics.db_pool_idle,
            db_pool_size: basics.db_pool_size,
            db_pool_max_connections: basics.db_pool_max_connections,
            db_pool_usage_basis_points: basics.db_pool_usage_basis_points,
            db_pool_under_maintenance_pressure: basics.db_pool_under_maintenance_pressure,
            postgres_active_connections: metric_max(samples, "postgres_active_connections"),
            postgres_waiting_connections: metric_max(samples, "postgres_waiting_connections"),
            postgres_lock_waiting_connections: metric_max(
                samples,
                "postgres_lock_waiting_connections",
            ),
            postgres_idle_in_transaction_connections: metric_max(
                samples,
                "postgres_idle_in_transaction_connections",
            ),
            postgres_oldest_active_query_age_ms: metric_max(
                samples,
                "postgres_oldest_active_query_age_ms",
            ),
            postgres_oldest_transaction_age_ms: metric_max(
                samples,
                "postgres_oldest_transaction_age_ms",
            ),
            gateway_requests_in_flight: find_metric_value_u64(
                samples,
                "concurrency_in_flight",
                &[("gate", "gateway_requests")],
            )
            .unwrap_or_default(),
            gateway_requests_distributed_in_flight: find_metric_value_u64(
                samples,
                "concurrency_in_flight",
                &[("gate", "gateway_requests_distributed")],
            )
            .unwrap_or_default(),
            gateway_auth_snapshot_load_in_flight: find_metric_value_u64(
                samples,
                "concurrency_in_flight",
                &[("gate", "gateway_auth_snapshot_load")],
            )
            .unwrap_or_default(),
            gateway_auth_snapshot_load_high_watermark: find_metric_value_u64(
                samples,
                "concurrency_high_watermark",
                &[("gate", "gateway_auth_snapshot_load")],
            )
            .unwrap_or_default(),
            gateway_candidate_planning_in_flight: find_metric_value_u64(
                samples,
                "concurrency_in_flight",
                &[("gate", "gateway_candidate_planning")],
            )
            .unwrap_or_default(),
            gateway_candidate_planning_high_watermark: find_metric_value_u64(
                samples,
                "concurrency_high_watermark",
                &[("gate", "gateway_candidate_planning")],
            )
            .unwrap_or_default(),
            request_candidate_queue_depth: metric_max(samples, "request_candidate_queue_depth"),
            request_candidate_queue_pending_depth: metric_max(
                samples,
                "request_candidate_queue_pending_depth",
            ),
            request_candidate_queue_db_write_in_flight: metric_max(
                samples,
                "request_candidate_queue_db_write_in_flight",
            ),
            request_candidate_queue_db_write_max_in_flight: metric_max(
                samples,
                "request_candidate_queue_db_write_max_in_flight",
            ),
            request_candidate_queue_db_write_wait_total: metric_max(
                samples,
                "request_candidate_queue_db_write_wait_total",
            ),
            request_candidate_queue_sync_fallback_total: metric_max(
                samples,
                "request_candidate_queue_sync_fallback_total",
            ),
            usage_runtime_worker_record_concurrency_in_flight: metric_max(
                samples,
                "usage_runtime_queue_worker_record_concurrency_in_flight",
            ),
            usage_runtime_worker_record_concurrency_max_in_flight: metric_max(
                samples,
                "usage_runtime_queue_worker_record_concurrency_max_in_flight",
            ),
            usage_runtime_worker_record_concurrency_wait_total: metric_max(
                samples,
                "usage_runtime_queue_worker_record_concurrency_wait_total",
            ),
            usage_runtime_worker_record_deferred_total: metric_max(
                samples,
                "usage_runtime_queue_worker_record_deferred_total",
            ),
            usage_queue_group_pending: metric_max(samples, "usage_queue_group_pending"),
            usage_queue_group_lag: metric_max(samples, "usage_queue_group_lag"),
            usage_counter_outbox_pending_rows: metric_max(
                samples,
                "usage_counter_outbox_pending_rows",
            ),
            usage_counter_outbox_oldest_pending_age_seconds: metric_max(
                samples,
                "usage_counter_outbox_oldest_pending_age_seconds",
            ),
            usage_counter_outbox_flush_rows_claimed_total: metric_max(
                samples,
                "usage_counter_outbox_flush_rows_claimed_total",
            ),
            usage_counter_outbox_flush_deferred_total: metric_max(
                samples,
                "usage_counter_outbox_flush_deferred_total",
            ),
            usage_counter_outbox_cleanup_deferred_total: metric_max(
                samples,
                "usage_counter_outbox_cleanup_deferred_total",
            ),
            redis_runtime_nonblocking_command_latency_ms: metric_max(
                samples,
                "redis_runtime_nonblocking_command_latency_ms",
            ),
            redis_runtime_lane_command_errors_total: metric_sum(
                samples,
                "redis_runtime_lane_command_errors_total",
            ),
            redis_runtime_lane_command_timeouts_total: metric_sum(
                samples,
                "redis_runtime_lane_command_timeouts_total",
            ),
            redis_runtime_connected_clients: metric_max(samples, "redis_runtime_connected_clients"),
            redis_runtime_blocked_clients: metric_max(samples, "redis_runtime_blocked_clients"),
            gateway_background_tasks_active: metric_max(samples, "gateway_background_tasks_active"),
            gateway_tokio_runtime_alive_tasks: metric_max(
                samples,
                "gateway_tokio_runtime_alive_tasks",
            ),
            gateway_tokio_runtime_global_queue_depth: metric_max(
                samples,
                "gateway_tokio_runtime_global_queue_depth",
            ),
            upstream_target_gate_in_flight: metric_max(samples, "upstream_target_gate_in_flight"),
            upstream_target_saturated_total: metric_sum(samples, "upstream_target_saturated_total"),
            active_background_task_keys: active_background_task_keys(samples),
            postgres_activity_groups: postgres_activity_groups(samples),
        }
    }
}

fn samples_are_drained(samples: &[PrometheusSample]) -> bool {
    metric_max(samples, "request_candidate_queue_depth") == 0
        && metric_max(samples, "request_candidate_queue_pending_depth") == 0
        && metric_max(samples, "usage_queue_group_pending") == 0
        && metric_max(samples, "usage_queue_group_lag") == 0
        && metric_max(samples, "usage_queue_dlq_length") == 0
        && metric_max(samples, "usage_runtime_enqueue_retry_pending") == 0
        && metric_max(samples, "usage_counter_outbox_pending_rows") == 0
        && metric_max(samples, "postgres_lock_waiting_connections") == 0
        && metric_max(samples, "postgres_idle_in_transaction_connections") == 0
}

async fn wait_for_settle_drain(
    metrics_url: &str,
    settle_after: Duration,
    sample_interval: Duration,
    summary: &Arc<Mutex<GatewayPressureMetricsSummary>>,
) -> SettleDrainResult {
    if settle_after.is_zero() {
        return SettleDrainResult::default();
    }

    let started = tokio::time::Instant::now();
    let poll_interval = sample_interval
        .min(Duration::from_millis(500))
        .max(Duration::from_millis(50));

    loop {
        match fetch_prometheus_samples(metrics_url).await {
            Ok(samples) => {
                let drained = samples_are_drained(&samples);
                summary.lock().await.observe(&samples);
                if drained {
                    return SettleDrainResult {
                        completed: true,
                        elapsed: started.elapsed(),
                    };
                }
            }
            Err(err) => eprintln!("gateway pressure probe settle metrics failed: {err}"),
        }

        let elapsed = started.elapsed();
        if elapsed >= settle_after {
            break;
        }
        tokio::time::sleep(poll_interval.min(settle_after.saturating_sub(elapsed))).await;
    }

    let mut completed = false;
    if let Ok(samples) = fetch_prometheus_samples(metrics_url).await {
        completed = samples_are_drained(&samples);
        summary.lock().await.observe(&samples);
    }

    SettleDrainResult {
        completed,
        elapsed: started.elapsed(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(std::env::args().skip(1).collect())?;
    let stop = Arc::new(AtomicBool::new(false));
    let summary = Arc::new(Mutex::new(GatewayPressureMetricsSummary::default()));

    match fetch_prometheus_samples(&config.metrics_url).await {
        Ok(samples) => summary.lock().await.observe(&samples),
        Err(err) => eprintln!("gateway pressure probe metrics baseline failed: {err}"),
    }

    let sampler = spawn_metrics_sampler(
        config.metrics_url.clone(),
        config.sample_interval,
        Arc::clone(&stop),
        Arc::clone(&summary),
    );

    let load = run_http_load_probe(&config.load)
        .await
        .map_err(std::io::Error::other)?;
    stop.store(true, Ordering::Release);
    sampler.await??;

    let settle_drain = wait_for_settle_drain(
        &config.metrics_url,
        config.settle_after,
        config.sample_interval,
        &summary,
    )
    .await;

    let report = GatewayPressureReport {
        suite: "gateway_pressure_probe",
        target_url: config.load.url,
        metrics_url: config.metrics_url,
        sample_interval_ms: config.sample_interval.as_millis() as u64,
        settle_after_ms: config.settle_after.as_millis() as u64,
        settle_drain_completed: settle_drain.completed,
        settle_drain_elapsed_ms: settle_drain.elapsed.as_millis() as u64,
        load,
        metrics: Arc::try_unwrap(summary)
            .unwrap_or_else(|_| panic!("metrics summary still referenced"))
            .into_inner(),
    };
    let raw = serde_json::to_string_pretty(&report)?;
    println!("{raw}");
    if let Some(path) = config.output_path.as_ref() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{raw}\n"))?;
    }
    Ok(())
}

fn spawn_metrics_sampler(
    metrics_url: String,
    interval: Duration,
    stop: Arc<AtomicBool>,
    summary: Arc<Mutex<GatewayPressureMetricsSummary>>,
) -> tokio::task::JoinHandle<Result<(), std::io::Error>> {
    tokio::spawn(async move {
        while !stop.load(Ordering::Acquire) {
            match fetch_prometheus_samples(&metrics_url).await {
                Ok(samples) => summary.lock().await.observe(&samples),
                Err(err) => {
                    eprintln!("gateway pressure probe metrics sample failed: {err}");
                }
            }
            tokio::time::sleep(interval).await;
        }
        Ok(())
    })
}

fn parse_args(args: Vec<String>) -> Result<Config, Box<dyn std::error::Error>> {
    let mut target_url: Option<String> = None;
    let mut warmup_url: Option<String> = None;
    let mut metrics_url: Option<String> = None;
    let mut total_requests: Option<usize> = None;
    let mut concurrency: Option<usize> = None;
    let mut warmup_connections: usize = 0;
    let mut timeout_ms: Option<u64> = None;
    let mut connect_timeout_ms: Option<u64> = None;
    let mut client_shards: Option<usize> = None;
    let mut pool_max_idle_per_host: Option<usize> = None;
    let mut start_ramp_ms: u64 = 0;
    let mut first_body_hold_ms: u64 = 0;
    let mut sample_interval_ms: u64 = 500;
    let mut settle_after_ms: u64 = 2_000;
    let mut method = Method::GET;
    let mut headers = BTreeMap::new();
    let mut api_key_list: Option<Vec<String>> = None;
    let mut body: Option<Vec<u8>> = None;
    let mut response_mode = HttpLoadProbeResponseMode::HeadersOnly;
    let mut http1_only = false;
    let mut http2_prior_knowledge = false;
    let mut output_path = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--url" => target_url = Some(next_value(&mut iter, "--url")?),
            "--warmup-url" => warmup_url = Some(next_value(&mut iter, "--warmup-url")?),
            "--metrics-url" => metrics_url = Some(next_value(&mut iter, "--metrics-url")?),
            "--requests" => total_requests = Some(next_value(&mut iter, "--requests")?.parse()?),
            "--concurrency" => concurrency = Some(next_value(&mut iter, "--concurrency")?.parse()?),
            "--warmup-connections" => {
                warmup_connections = next_value(&mut iter, "--warmup-connections")?.parse()?
            }
            "--timeout-ms" => timeout_ms = Some(next_value(&mut iter, "--timeout-ms")?.parse()?),
            "--connect-timeout-ms" => {
                connect_timeout_ms = Some(next_value(&mut iter, "--connect-timeout-ms")?.parse()?)
            }
            "--client-shards" => {
                client_shards = Some(next_value(&mut iter, "--client-shards")?.parse()?)
            }
            "--pool-max-idle-per-host" => {
                pool_max_idle_per_host =
                    Some(next_value(&mut iter, "--pool-max-idle-per-host")?.parse()?)
            }
            "--start-ramp-ms" => {
                start_ramp_ms = next_value(&mut iter, "--start-ramp-ms")?.parse()?
            }
            "--first-body-hold-ms" => {
                first_body_hold_ms = next_value(&mut iter, "--first-body-hold-ms")?.parse()?
            }
            "--http1-only" => http1_only = true,
            "--http2-prior-knowledge" => http2_prior_knowledge = true,
            "--sample-interval-ms" => {
                sample_interval_ms = next_value(&mut iter, "--sample-interval-ms")?.parse()?
            }
            "--settle-after-ms" => {
                settle_after_ms = next_value(&mut iter, "--settle-after-ms")?.parse()?
            }
            "--method" => {
                method = Method::from_bytes(next_value(&mut iter, "--method")?.as_bytes())?
            }
            "--header" | "-H" => {
                let (name, value) = parse_header_arg(&next_value(&mut iter, "--header")?)?;
                headers.insert(name, value);
            }
            "--api-key-file" => {
                let api_key = read_secret_file(&next_value(&mut iter, "--api-key-file")?)?;
                headers.insert("Authorization".to_string(), format!("Bearer {api_key}"));
            }
            "--api-key-list-file" => {
                api_key_list = Some(read_secret_list_file(&next_value(
                    &mut iter,
                    "--api-key-list-file",
                )?)?);
            }
            "--body" => body = Some(next_value(&mut iter, "--body")?.into_bytes()),
            "--body-file" => body = Some(std::fs::read(next_value(&mut iter, "--body-file")?)?),
            "--response-mode" => {
                response_mode = parse_response_mode(&next_value(&mut iter, "--response-mode")?)?
            }
            "--output" => output_path = Some(PathBuf::from(next_value(&mut iter, "--output")?)),
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown argument: {other}"),
                )
                .into());
            }
        }
    }

    let mut load = HttpLoadProbeConfig {
        url: target_url.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing required --url")
        })?,
        total_requests: total_requests.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing required --requests",
            )
        })?,
        concurrency: concurrency.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing required --concurrency",
            )
        })?,
        method,
        headers,
        body,
        response_mode,
        ..HttpLoadProbeConfig::default()
    };
    if let Some(api_keys) = api_key_list {
        load.header_sets = api_keys
            .into_iter()
            .map(|api_key| {
                let mut headers = load.headers.clone();
                headers.insert("Authorization".to_string(), format!("Bearer {api_key}"));
                headers
            })
            .collect();
    }
    load.warmup_url = warmup_url;
    load.warmup_connections = warmup_connections;
    if let Some(timeout_ms) = timeout_ms {
        load.timeout = Duration::from_millis(timeout_ms);
    }
    load.connect_timeout = connect_timeout_ms.map(Duration::from_millis);
    if let Some(client_shards) = client_shards {
        load.client_shards = client_shards;
    }
    load.pool_max_idle_per_host = pool_max_idle_per_host;
    load.start_ramp = Duration::from_millis(start_ramp_ms);
    load.first_body_hold = Duration::from_millis(first_body_hold_ms);
    load.http1_only = http1_only;
    load.http2_prior_knowledge = http2_prior_knowledge;
    load.validate()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    if sample_interval_ms == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--sample-interval-ms must be positive",
        )
        .into());
    }
    Ok(Config {
        load,
        metrics_url: metrics_url.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing required --metrics-url",
            )
        })?,
        sample_interval: Duration::from_millis(sample_interval_ms),
        settle_after: Duration::from_millis(settle_after_ms),
        output_path,
    })
}

fn parse_header_arg(value: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let (name, value) = value
        .split_once(':')
        .or_else(|| value.split_once('='))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--header expects `Name: value` or `Name=value`",
            )
        })?;
    let name = name.trim();
    if name.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--header name cannot be empty",
        )
        .into());
    }
    Ok((name.to_string(), value.trim().to_string()))
}

fn read_secret_file(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let secret = fs::read_to_string(path)?;
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{path} is empty"),
        )
        .into());
    }
    Ok(secret.to_string())
}

fn read_secret_list_file(path: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let secrets = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if secrets.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{path} does not contain any API keys"),
        )
        .into());
    }
    Ok(secrets)
}

fn parse_response_mode(
    value: &str,
) -> Result<HttpLoadProbeResponseMode, Box<dyn std::error::Error>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "headers" | "headers-only" | "header" => Ok(HttpLoadProbeResponseMode::HeadersOnly),
        "first-body-byte" | "first-body" | "first-byte" | "first-chunk" => {
            Ok(HttpLoadProbeResponseMode::FirstBodyByte)
        }
        "full" | "full-body" | "body" => Ok(HttpLoadProbeResponseMode::FullBody),
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "unsupported --response-mode {other}; expected headers, first-body-byte, or full"
            ),
        )
        .into()),
    }
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    iter.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("missing value for {flag}"),
        )
        .into()
    })
}

fn metric_max(samples: &[PrometheusSample], metric_name: &str) -> u64 {
    samples
        .iter()
        .filter(|sample| metric_name_matches(&sample.name, metric_name))
        .filter_map(|sample| sample.value.parse::<u64>().ok())
        .max()
        .unwrap_or_default()
}

fn metric_min(samples: &[PrometheusSample], metric_name: &str) -> Option<u64> {
    samples
        .iter()
        .filter(|sample| metric_name_matches(&sample.name, metric_name))
        .filter_map(|sample| sample.value.parse::<u64>().ok())
        .min()
}

fn metric_sum(samples: &[PrometheusSample], metric_name: &str) -> u64 {
    samples
        .iter()
        .filter(|sample| metric_name_matches(&sample.name, metric_name))
        .filter_map(|sample| sample.value.parse::<u64>().ok())
        .sum()
}

fn metric_for_lane(samples: &[PrometheusSample], metric_name: &str, lane: &str) -> u64 {
    find_metric_value_u64(samples, metric_name, &[("lane", lane)]).unwrap_or_default()
}

fn metric_for_lane_and_bucket(
    samples: &[PrometheusSample],
    metric_name: &str,
    lane: &str,
    upper_bound_ms: &str,
) -> u64 {
    find_metric_value_u64(
        samples,
        metric_name,
        &[("lane", lane), ("le", upper_bound_ms)],
    )
    .unwrap_or_default()
}

fn db_pool_pressure_window_triggered(
    checked_out: u64,
    max_connections: u64,
    usage_basis_points: u64,
    under_maintenance_pressure: u64,
) -> bool {
    under_maintenance_pressure > 0
        || usage_basis_points >= DB_POOL_PRESSURE_USAGE_BASIS_POINTS
        || (max_connections > 0 && checked_out >= max_connections)
}

fn active_background_task_keys(samples: &[PrometheusSample]) -> Vec<String> {
    let mut keys = samples
        .iter()
        .filter(|sample| metric_name_matches(&sample.name, "gateway_background_task_active"))
        .filter(|sample| sample.value.parse::<u64>().unwrap_or_default() > 0)
        .filter_map(|sample| sample.labels.get("task_key").cloned())
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys
}

fn postgres_activity_groups(samples: &[PrometheusSample]) -> Vec<GatewayPostgresActivityGroup> {
    let mut groups = samples
        .iter()
        .filter(|sample| metric_name_matches(&sample.name, "postgres_activity_group_connections"))
        .filter_map(|sample| {
            let rank_label = sample.labels.get("rank")?;
            let connections = sample.value.parse::<u64>().ok()?;
            let rank = rank_label.parse::<u64>().ok()?;
            let state = sample.labels.get("state")?.clone();
            let wait_event_type = sample.labels.get("wait_event_type")?.clone();
            let wait_event = sample.labels.get("wait_event")?.clone();
            let query_prefix = sample.labels.get("query_prefix")?.clone();
            let max_query_age_ms = find_metric_value_u64(
                samples,
                "postgres_activity_group_max_query_age_ms",
                &[("rank", rank_label.as_str())],
            )
            .unwrap_or_default();
            let max_transaction_age_ms = find_metric_value_u64(
                samples,
                "postgres_activity_group_max_transaction_age_ms",
                &[("rank", rank_label.as_str())],
            )
            .unwrap_or_default();

            Some(GatewayPostgresActivityGroup {
                rank,
                state,
                wait_event_type,
                wait_event,
                query_prefix,
                connections,
                max_query_age_ms,
                max_transaction_age_ms,
            })
        })
        .collect::<Vec<_>>();
    groups.sort_by_key(|group| group.rank);
    groups
}

fn delta_from_first(first: Option<u64>, current: u64) -> u64 {
    first
        .map(|first| current.saturating_sub(first))
        .unwrap_or_default()
}

fn metric_name_matches(actual: &str, expected: &str) -> bool {
    actual == expected
        || actual.strip_prefix("aether-gateway_") == Some(expected)
        || actual.strip_prefix("aether_gateway_") == Some(expected)
}

fn print_usage() {
    eprintln!(
        "usage: cargo run -p aether-loadtools --bin gateway_pressure_probe -- --url <URL> --metrics-url <URL> --requests <N> --concurrency <N> [--warmup-url <URL>] [--warmup-connections N] [--method GET] [--timeout-ms 30000] [--connect-timeout-ms 10000] [--client-shards 1] [--pool-max-idle-per-host N] [--start-ramp-ms 0] [--first-body-hold-ms 0] [--http1-only | --http2-prior-knowledge] [--sample-interval-ms 500] [-H 'Name: value'] [--api-key-file path | --api-key-list-file path] [--body JSON | --body-file path] [--response-mode headers|first-body-byte|full] [--output /tmp/gateway_pressure.json]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn metric_aggregation_does_not_mix_foreground_and_background_pools() {
        let samples = vec![
            sample("aether-gateway_database_pool_usage_basis_points", 1_429),
            sample(
                "aether-gateway_background_database_pool_usage_basis_points",
                10_000,
            ),
        ];

        assert_eq!(
            metric_max(&samples, "database_pool_usage_basis_points"),
            1_429
        );
    }

    fn sample(name: &str, value: u64) -> PrometheusSample {
        PrometheusSample {
            name: name.to_string(),
            labels: BTreeMap::new(),
            value: value.to_string(),
        }
    }

    fn lane_sample(name: &str, lane: &str, value: u64) -> PrometheusSample {
        PrometheusSample {
            name: name.to_string(),
            labels: BTreeMap::from([("lane".to_string(), lane.to_string())]),
            value: value.to_string(),
        }
    }

    fn labeled_sample(name: &str, labels: &[(&str, &str)], value: u64) -> PrometheusSample {
        PrometheusSample {
            name: name.to_string(),
            labels: labels
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            value: value.to_string(),
        }
    }

    #[test]
    fn postgres_statement_delta_baseline_ignores_unavailable_scrapes() {
        let mut summary = GatewayPressureMetricsSummary::default();

        summary.observe(&[
            sample("postgres_statement_observability_available", 0),
            sample("postgres_statement_observability_unavailable", 1),
            sample("postgres_statement_top_max_exec_time_ms", 0),
        ]);
        assert_eq!(summary.postgres_first_statement_top_max_exec_time_ms, None);

        summary.observe(&[
            sample("postgres_statement_observability_available", 1),
            sample("postgres_statement_observability_unavailable", 0),
            sample("postgres_statement_top_max_exec_time_ms", 74_104),
        ]);
        assert_eq!(
            summary.postgres_first_statement_top_max_exec_time_ms,
            Some(74_104)
        );
        assert_eq!(summary.postgres_statement_top_max_exec_time_ms_delta, 0);

        summary.observe(&[
            sample("postgres_statement_observability_available", 1),
            sample("postgres_statement_observability_unavailable", 0),
            sample("postgres_statement_top_max_exec_time_ms", 75_000),
        ]);
        assert_eq!(summary.postgres_statement_top_max_exec_time_ms_delta, 896);
    }

    #[test]
    fn redis_delta_baselines_ignore_unavailable_scrapes() {
        let mut summary = GatewayPressureMetricsSummary::default();

        summary.observe(&[
            sample("redis_runtime_enabled", 1),
            sample("redis_runtime_health_unavailable", 1),
            sample("redis_runtime_total_error_replies", 0),
            lane_sample("redis_runtime_lane_command_errors_total", "fast", 0),
        ]);
        assert_eq!(summary.redis_runtime_first_total_error_replies, None);
        assert_eq!(summary.redis_runtime_first_lane_command_errors_total, None);

        summary.observe(&[
            sample("redis_runtime_enabled", 1),
            sample("redis_runtime_health_unavailable", 0),
            sample("redis_runtime_total_error_replies", 158),
            lane_sample("redis_runtime_lane_command_errors_total", "fast", 1),
        ]);
        assert_eq!(summary.redis_runtime_first_total_error_replies, Some(158));
        assert_eq!(summary.redis_runtime_total_error_replies_delta, 0);
        assert_eq!(
            summary.redis_runtime_first_lane_command_errors_total,
            Some(1)
        );
        assert_eq!(summary.redis_runtime_lane_command_errors_total_delta, 0);

        summary.observe(&[
            sample("redis_runtime_enabled", 1),
            sample("redis_runtime_health_unavailable", 0),
            sample("redis_runtime_total_error_replies", 160),
            lane_sample("redis_runtime_lane_command_errors_total", "fast", 3),
        ]);
        assert_eq!(summary.redis_runtime_total_error_replies_delta, 2);
        assert_eq!(summary.redis_runtime_lane_command_errors_total_delta, 2);
    }

    #[test]
    fn redis_latency_delta_ignores_existing_max_values() {
        let mut summary = GatewayPressureMetricsSummary::default();

        summary.observe(&[
            sample("redis_runtime_enabled", 1),
            sample("redis_runtime_health_unavailable", 0),
            lane_sample("redis_runtime_lane_command_latency_ms_max", "fast", 100),
            lane_sample("redis_runtime_lane_command_latency_ms_max", "stream", 5_017),
            lane_sample("redis_runtime_lane_command_latency_ms_max", "admin", 71),
        ]);
        assert_eq!(
            summary.redis_runtime_first_nonblocking_command_latency_ms,
            Some(5_017)
        );
        assert_eq!(
            summary.redis_runtime_nonblocking_command_latency_ms_delta,
            0
        );

        summary.observe(&[
            sample("redis_runtime_enabled", 1),
            sample("redis_runtime_health_unavailable", 0),
            lane_sample("redis_runtime_lane_command_latency_ms_max", "fast", 100),
            lane_sample("redis_runtime_lane_command_latency_ms_max", "stream", 5_250),
            lane_sample("redis_runtime_lane_command_latency_ms_max", "admin", 71),
        ]);
        assert_eq!(
            summary.redis_runtime_max_nonblocking_command_latency_ms,
            5_250
        );
        assert_eq!(
            summary.redis_runtime_nonblocking_command_latency_ms_delta,
            233
        );
    }

    #[test]
    fn redis_nonblocking_slow_command_rate_uses_histogram_deltas() {
        let mut summary = GatewayPressureMetricsSummary::default();

        let histogram = |lane: &str, le: &str, value: u64| {
            labeled_sample(
                "redis_runtime_lane_command_latency_ms_bucket",
                &[("lane", lane), ("le", le)],
                value,
            )
        };
        summary.observe(&[
            sample("redis_runtime_enabled", 1),
            sample("redis_runtime_health_unavailable", 0),
            histogram("fast", "500", 10),
            histogram("fast", "+Inf", 10),
            histogram("stream", "500", 100),
            histogram("stream", "+Inf", 101),
            histogram("admin", "500", 20),
            histogram("admin", "+Inf", 20),
        ]);
        summary.observe(&[
            sample("redis_runtime_enabled", 1),
            sample("redis_runtime_health_unavailable", 0),
            histogram("fast", "500", 110),
            histogram("fast", "+Inf", 110),
            histogram("stream", "500", 10_000),
            histogram("stream", "+Inf", 10_008),
            histogram("admin", "500", 120),
            histogram("admin", "+Inf", 121),
        ]);

        assert_eq!(
            summary.redis_runtime_nonblocking_command_count_total_delta,
            10_108
        );
        assert_eq!(
            summary.redis_runtime_nonblocking_command_le_500ms_total_delta,
            10_100
        );
        assert_eq!(
            summary.redis_runtime_nonblocking_command_over_500ms_total_delta,
            8
        );
        assert_eq!(
            summary.redis_runtime_nonblocking_command_over_500ms_rate_basis_points,
            7
        );
    }

    #[test]
    fn usage_failure_deltas_ignore_existing_counter_values() {
        let mut summary = GatewayPressureMetricsSummary::default();

        summary.observe(&[
            sample("usage_runtime_queue_worker_process_failures_total", 2),
            sample("usage_runtime_queue_worker_read_failures_total", 3),
            sample("usage_runtime_queue_worker_reclaim_failures_total", 4),
            sample("usage_counter_outbox_flush_failed_batches_total", 5),
            sample("usage_counter_outbox_cleanup_failed_batches_total", 6),
        ]);
        assert_eq!(summary.usage_runtime_worker_process_failures_total_delta, 0);
        assert_eq!(summary.usage_runtime_worker_read_failures_total_delta, 0);
        assert_eq!(summary.usage_runtime_worker_reclaim_failures_total_delta, 0);
        assert_eq!(
            summary.usage_counter_outbox_flush_failed_batches_total_delta,
            0
        );
        assert_eq!(
            summary.usage_counter_outbox_cleanup_failed_batches_total_delta,
            0
        );

        summary.observe(&[
            sample("usage_runtime_queue_worker_process_failures_total", 3),
            sample("usage_runtime_queue_worker_read_failures_total", 5),
            sample("usage_runtime_queue_worker_reclaim_failures_total", 7),
            sample("usage_counter_outbox_flush_failed_batches_total", 9),
            sample("usage_counter_outbox_cleanup_failed_batches_total", 11),
        ]);
        assert_eq!(summary.usage_runtime_worker_process_failures_total_delta, 1);
        assert_eq!(summary.usage_runtime_worker_read_failures_total_delta, 2);
        assert_eq!(summary.usage_runtime_worker_reclaim_failures_total_delta, 3);
        assert_eq!(
            summary.usage_counter_outbox_flush_failed_batches_total_delta,
            4
        );
        assert_eq!(
            summary.usage_counter_outbox_cleanup_failed_batches_total_delta,
            5
        );
    }

    #[test]
    fn db_pool_pressure_window_triggers_on_usage_or_full_pool() {
        assert!(db_pool_pressure_window_triggered(64, 64, 8_500, 0));
        assert!(db_pool_pressure_window_triggered(63, 64, 9_000, 0));
        assert!(db_pool_pressure_window_triggered(1, 64, 1, 1));
        assert!(!db_pool_pressure_window_triggered(32, 64, 8_000, 0));
    }

    #[test]
    fn pressure_window_captures_active_background_tasks_and_queue_fields() {
        let samples = vec![
            sample("database_pool_checked_out_connections", 64),
            sample("database_pool_idle_connections", 0),
            sample("database_pool_size_connections", 64),
            sample("database_pool_max_connections", 64),
            sample("database_pool_usage_basis_points", 10_000),
            sample("database_pool_under_maintenance_pressure", 1),
            sample("postgres_active_connections", 8),
            sample("postgres_waiting_connections", 5),
            sample("postgres_lock_waiting_connections", 1),
            sample("postgres_idle_in_transaction_connections", 0),
            sample("postgres_oldest_active_query_age_ms", 123),
            sample("postgres_oldest_transaction_age_ms", 456),
            sample("concurrency_in_flight", 9),
            labeled_sample("concurrency_in_flight", &[("gate", "gateway_requests")], 7),
            labeled_sample(
                "concurrency_in_flight",
                &[("gate", "gateway_requests_distributed")],
                3,
            ),
            labeled_sample(
                "concurrency_in_flight",
                &[("gate", "gateway_auth_snapshot_load")],
                2,
            ),
            labeled_sample(
                "concurrency_high_watermark",
                &[("gate", "gateway_auth_snapshot_load")],
                4,
            ),
            labeled_sample(
                "concurrency_in_flight",
                &[("gate", "gateway_candidate_planning")],
                6,
            ),
            labeled_sample(
                "concurrency_high_watermark",
                &[("gate", "gateway_candidate_planning")],
                8,
            ),
            sample("request_candidate_queue_depth", 11),
            sample("request_candidate_queue_pending_depth", 9),
            sample("request_candidate_queue_db_write_in_flight", 2),
            sample("request_candidate_queue_db_write_max_in_flight", 4),
            sample("request_candidate_queue_db_write_wait_total", 13),
            sample("request_candidate_queue_sync_fallback_total", 3),
            sample("usage_runtime_queue_worker_record_concurrency_in_flight", 1),
            sample(
                "usage_runtime_queue_worker_record_concurrency_max_in_flight",
                5,
            ),
            sample(
                "usage_runtime_queue_worker_record_concurrency_wait_total",
                7,
            ),
            sample("usage_runtime_queue_worker_record_deferred_total", 9),
            sample("usage_queue_group_pending", 12),
            sample("usage_queue_group_lag", 4),
            sample("usage_counter_outbox_pending_rows", 15),
            sample("usage_counter_outbox_oldest_pending_age_seconds", 16),
            sample("usage_counter_outbox_flush_rows_claimed_total", 18),
            sample("usage_counter_outbox_flush_deferred_total", 2),
            sample("usage_counter_outbox_cleanup_deferred_total", 1),
            sample("redis_runtime_nonblocking_command_latency_ms", 29),
            sample("redis_runtime_lane_command_errors_total", 4),
            sample("redis_runtime_lane_command_timeouts_total", 5),
            sample("redis_runtime_connected_clients", 23),
            sample("redis_runtime_blocked_clients", 2),
            sample("gateway_background_tasks_active", 6),
            sample("gateway_tokio_runtime_alive_tasks", 32),
            sample("gateway_tokio_runtime_global_queue_depth", 4),
            sample("upstream_target_gate_in_flight", 13),
            sample("upstream_target_saturated_total", 1),
            labeled_sample(
                "gateway_background_task_active",
                &[("task_key", "usage_counter_flush")],
                1,
            ),
            labeled_sample(
                "gateway_background_task_active",
                &[("task_key", "audit_cleanup")],
                0,
            ),
            labeled_sample(
                "gateway_background_task_active",
                &[("task_key", "pool_score_rebuild")],
                1,
            ),
            labeled_sample(
                "postgres_activity_group_connections",
                &[
                    ("rank", "2"),
                    ("state", "idle in transaction"),
                    ("wait_event_type", "Client"),
                    ("wait_event", "ClientRead"),
                    ("query_prefix", "COMMIT"),
                ],
                3,
            ),
            labeled_sample(
                "postgres_activity_group_max_query_age_ms",
                &[("rank", "2")],
                17,
            ),
            labeled_sample(
                "postgres_activity_group_max_transaction_age_ms",
                &[("rank", "2")],
                88,
            ),
            labeled_sample(
                "postgres_activity_group_connections",
                &[
                    ("rank", "1"),
                    ("state", "active"),
                    ("wait_event_type", "Lock"),
                    ("wait_event", "transactionid"),
                    ("query_prefix", "INSERT INTO usage"),
                ],
                5,
            ),
            labeled_sample(
                "postgres_activity_group_max_query_age_ms",
                &[("rank", "1")],
                44,
            ),
            labeled_sample(
                "postgres_activity_group_max_transaction_age_ms",
                &[("rank", "1")],
                66,
            ),
        ];

        let window = GatewayDbPoolPressureWindow::from_samples(
            &samples,
            GatewayDbPoolPressureBasics {
                sample_index: 7,
                db_pool_checked_out: 64,
                db_pool_idle: Some(0),
                db_pool_size: 64,
                db_pool_max_connections: 64,
                db_pool_usage_basis_points: 10_000,
                db_pool_under_maintenance_pressure: 1,
            },
        );

        assert_eq!(window.sample_index, 7);
        assert_eq!(window.db_pool_checked_out, 64);
        assert_eq!(window.gateway_auth_snapshot_load_high_watermark, 4);
        assert_eq!(window.gateway_candidate_planning_high_watermark, 8);
        assert_eq!(
            window.active_background_task_keys,
            vec![
                "pool_score_rebuild".to_string(),
                "usage_counter_flush".to_string()
            ]
        );
        assert_eq!(window.usage_counter_outbox_pending_rows, 15);
        assert_eq!(window.request_candidate_queue_db_write_wait_total, 13);
        assert_eq!(window.gateway_requests_in_flight, 7);
        assert_eq!(window.usage_runtime_worker_record_deferred_total, 9);
        assert_eq!(window.postgres_activity_groups.len(), 2);
        assert_eq!(window.postgres_activity_groups[0].rank, 1);
        assert_eq!(window.postgres_activity_groups[0].state, "active");
        assert_eq!(
            window.postgres_activity_groups[0].query_prefix,
            "INSERT INTO usage"
        );
        assert_eq!(window.postgres_activity_groups[0].connections, 5);
        assert_eq!(window.postgres_activity_groups[0].max_query_age_ms, 44);
        assert_eq!(window.postgres_activity_groups[1].rank, 2);
        assert_eq!(
            window.postgres_activity_groups[1].state,
            "idle in transaction"
        );
    }

    #[test]
    fn settle_drain_requires_background_queues_and_postgres_waits_to_clear() {
        let drained = [
            sample("request_candidate_queue_depth", 0),
            sample("request_candidate_queue_pending_depth", 0),
            sample("usage_queue_group_pending", 0),
            sample("usage_queue_group_lag", 0),
            sample("usage_queue_dlq_length", 0),
            sample("usage_runtime_enqueue_retry_pending", 0),
            sample("usage_counter_outbox_pending_rows", 0),
            sample("postgres_lock_waiting_connections", 0),
            sample("postgres_idle_in_transaction_connections", 0),
        ];
        assert!(samples_are_drained(&drained));

        let pending_usage = [
            sample("request_candidate_queue_depth", 0),
            sample("request_candidate_queue_pending_depth", 0),
            sample("usage_queue_group_pending", 1),
            sample("usage_queue_group_lag", 0),
            sample("usage_queue_dlq_length", 0),
            sample("usage_runtime_enqueue_retry_pending", 0),
            sample("usage_counter_outbox_pending_rows", 0),
            sample("postgres_lock_waiting_connections", 0),
            sample("postgres_idle_in_transaction_connections", 0),
        ];
        assert!(!samples_are_drained(&pending_usage));

        let lock_wait = [
            sample("request_candidate_queue_depth", 0),
            sample("request_candidate_queue_pending_depth", 0),
            sample("usage_queue_group_pending", 0),
            sample("usage_queue_group_lag", 0),
            sample("usage_queue_dlq_length", 0),
            sample("usage_runtime_enqueue_retry_pending", 0),
            sample("usage_counter_outbox_pending_rows", 0),
            sample("postgres_lock_waiting_connections", 1),
            sample("postgres_idle_in_transaction_connections", 0),
        ];
        assert!(!samples_are_drained(&lock_wait));

        let pending_local_dispatch = [
            sample("request_candidate_queue_depth", 0),
            sample("request_candidate_queue_pending_depth", 0),
            sample("usage_queue_group_pending", 0),
            sample("usage_queue_group_lag", 0),
            sample("usage_queue_dlq_length", 0),
            sample("usage_runtime_enqueue_retry_pending", 1),
            sample("usage_counter_outbox_pending_rows", 0),
            sample("postgres_lock_waiting_connections", 0),
            sample("postgres_idle_in_transaction_connections", 0),
        ];
        assert!(!samples_are_drained(&pending_local_dispatch));
    }
}
