use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Weekday;

use crate::admin_api::admin_provider_ops_local_action_response;
use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

#[path = "runtime/account_self_check.rs"]
mod account_self_check;
#[path = "runtime/audit_cleanup.rs"]
mod audit_cleanup;
#[path = "runtime/cleanup_runs.rs"]
mod cleanup_runs;
#[path = "runtime/config.rs"]
mod config;
#[path = "runtime/db_maintenance.rs"]
mod db_maintenance;
#[path = "runtime/oauth_token_refresh.rs"]
mod oauth_token_refresh;
#[path = "runtime/pending_cleanup.rs"]
mod pending_cleanup;
#[path = "runtime/pool_quota_probe.rs"]
mod pool_quota_probe;
#[path = "runtime/pool_score_rebuild.rs"]
mod pool_score_rebuild;
#[path = "runtime/provider_checkin.rs"]
mod provider_checkin;
#[path = "runtime/provider_quota_alert.rs"]
mod provider_quota_alert;
#[path = "runtime/proxy_node_metrics_cleanup.rs"]
mod proxy_node_metrics_cleanup;
#[path = "runtime/proxy_node_staleness.rs"]
mod proxy_node_staleness;
#[path = "runtime/proxy_upgrade_rollout.rs"]
mod proxy_upgrade_rollout;
#[path = "runtime/request_candidate_cleanup.rs"]
mod request_candidate_cleanup;
#[path = "runtime/runners.rs"]
mod runners;
#[path = "runtime/schedule.rs"]
mod schedule;
#[path = "runtime/stats_daily.rs"]
mod stats_daily;
#[path = "runtime/stats_hourly.rs"]
mod stats_hourly;
#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
#[path = "runtime/usage_cleanup.rs"]
mod usage_cleanup;
#[path = "runtime/usage_counter_flush.rs"]
mod usage_counter_flush;
#[path = "runtime/wallet_daily_usage.rs"]
mod wallet_daily_usage;
#[path = "runtime/workers.rs"]
mod workers;
pub(crate) use account_self_check::{
    perform_account_self_check_once, perform_account_self_check_once_with_config,
    select_account_self_check_key_ids, spawn_account_self_check_worker, AccountSelfCheckRunSummary,
    AccountSelfCheckWorkerConfig,
};
pub(crate) use aether_data_contracts::repository::usage::{
    UsageCleanupSummary, UsageCleanupWindow,
};
use audit_cleanup::*;
pub(crate) use cleanup_runs::{
    list_admin_cleanup_run_records, record_admin_cleanup_run, record_completed_cleanup_run,
    record_failed_cleanup_run, start_admin_request_body_cleanup_task,
    start_admin_system_purge_task, AdminCleanupRunRecord, AdminCleanupTaskKind, USAGE_CLEANUP_KIND,
};
use config::*;
use db_maintenance::*;
pub(crate) use oauth_token_refresh::{
    perform_oauth_token_refresh_once, OAuthTokenRefreshRunSummary,
};
use pending_cleanup::*;
pub(crate) use pool_quota_probe::{
    perform_pool_quota_probe_once, perform_pool_quota_probe_once_for_provider_with_config,
    perform_pool_quota_probe_once_with_config, pool_quota_probe_target_count,
    select_pool_quota_probe_key_ids, spawn_pool_quota_probe_replenish_for_request,
    spawn_pool_quota_probe_worker, PoolQuotaProbeRunSummary, PoolQuotaProbeWorkerConfig,
};
pub(crate) use pool_score_rebuild::{
    ensure_provider_key_pool_scores_for_keys, perform_pool_score_rebuild_once,
    perform_pool_score_rebuild_once_with_config, spawn_pool_score_rebuild_worker,
    PoolScoreRebuildRunSummary, PoolScoreRebuildWorkerConfig,
};
pub(crate) use provider_checkin::{perform_provider_checkin_once, ProviderCheckinRunSummary};
pub(crate) use provider_quota_alert::{
    perform_provider_quota_alert_once, ProviderQuotaAlertRunSummary,
};
use proxy_node_metrics_cleanup::*;
use proxy_node_staleness::*;
use proxy_upgrade_rollout::*;
pub(crate) use proxy_upgrade_rollout::{
    cancel_proxy_upgrade_rollout, clear_proxy_upgrade_rollout_conflicts,
    collect_proxy_upgrade_rollout_probes, inspect_proxy_upgrade_rollout,
    record_proxy_upgrade_traffic_success, restore_proxy_upgrade_rollout_skipped_nodes,
    retry_proxy_upgrade_rollout_node, skip_proxy_upgrade_rollout_node, start_proxy_upgrade_rollout,
    ProxyUpgradeRolloutCancelSummary, ProxyUpgradeRolloutConflictClearSummary,
    ProxyUpgradeRolloutNodeActionSummary, ProxyUpgradeRolloutPendingProbe,
    ProxyUpgradeRolloutProbeConfig, ProxyUpgradeRolloutSkippedRestoreSummary,
    ProxyUpgradeRolloutStatus, ProxyUpgradeRolloutSummary, ProxyUpgradeRolloutTrackedNodeState,
};
use request_candidate_cleanup::*;
use runners::*;
pub(crate) use runners::{
    run_manual_usage_cleanup_once, start_manual_usage_cleanup_task, ManualUsageCleanupError,
};
use schedule::*;
use stats_daily::*;
use stats_hourly::*;
use usage_cleanup::*;
pub(crate) use usage_cleanup::{
    preview_manual_usage_cleanup, ManualUsageCleanupMode, ManualUsageCleanupOptions,
};
use usage_counter_flush::*;
pub(crate) use usage_counter_flush::{
    UsageCounterFlushRuntimeMetrics, UsageCounterFlushWorkerConfig,
};
use wallet_daily_usage::*;
pub(crate) use workers::*;

pub(super) fn postgres_error(
    error: impl std::fmt::Display,
) -> aether_data_contracts::DataLayerError {
    aether_data_contracts::DataLayerError::postgres(error)
}

const AUDIT_LOG_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const GEMINI_FILE_MAPPING_CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const PENDING_CLEANUP_INTERVAL: Duration = Duration::from_secs(5 * 60);
const PROXY_NODE_STALE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const PROXY_NODE_METRICS_CLEANUP_HOUR: u32 = 2;
const PROXY_NODE_METRICS_CLEANUP_MINUTE: u32 = 10;
const PROXY_UPGRADE_ROLLOUT_INTERVAL: Duration = Duration::from_secs(15);
const PROXY_NODE_STALE_MIN_GRACE_SECS: u64 = 15;
const PROXY_NODE_STALE_MISSED_HEARTBEATS: u64 = 3;
const POOL_MONITOR_INTERVAL: Duration = Duration::from_secs(5 * 60);
const OAUTH_TOKEN_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const PROVIDER_CHECKIN_CONCURRENCY: usize = 3;
const PROVIDER_QUOTA_ALERT_CONCURRENCY: usize = 3;
const PROVIDER_QUOTA_ALERT_INTERVAL: Duration = Duration::from_secs(5);
const PROVIDER_CHECKIN_DEFAULT_TIME: &str = "01:05";
const REQUEST_CANDIDATE_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const STATS_DAILY_AGGREGATION_HOUR: u32 = 0;
const STATS_DAILY_AGGREGATION_MINUTE: u32 = 5;
const STATS_HOURLY_AGGREGATION_MINUTE: u32 = 5;
const USAGE_CLEANUP_HOUR: u32 = 3;
const USAGE_CLEANUP_MINUTE: u32 = 0;
const WALLET_DAILY_USAGE_AGGREGATION_HOUR: u32 = 0;
const WALLET_DAILY_USAGE_AGGREGATION_MINUTE: u32 = 10;
const DB_MAINTENANCE_WEEKLY_INTERVAL: chrono::Duration = chrono::Duration::days(7);
const DB_MAINTENANCE_WEEKDAY: Weekday = Weekday::Sun;
const DB_MAINTENANCE_HOUR: u32 = 5;
const DB_MAINTENANCE_MINUTE: u32 = 0;
const MAINTENANCE_DEFAULT_TIMEZONE: &str = "Asia/Shanghai";
const DB_MAINTENANCE_TABLES: &[&str] = &["usage", "request_candidates", "audit_logs"];
const MAX_ADMIN_STATS_REBUILD_BUCKETS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsageCleanupSettings {
    detail_retention_days: u64,
    compressed_retention_days: u64,
    header_retention_days: u64,
    log_retention_days: u64,
    batch_size: usize,
    auto_delete_expired_keys: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct AdminSystemCleanupSummary {
    pub(crate) audit_logs_deleted: usize,
    pub(crate) request_candidates_deleted: usize,
    pub(crate) proxy_node_metrics:
        aether_data::repository::proxy_nodes::ProxyNodeMetricsCleanupSummary,
    pub(crate) pending_failed: usize,
    pub(crate) pending_recovered: usize,
    pub(crate) usage: UsageCleanupSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct AdminStatsRebuildSummary {
    pub(crate) hourly_buckets: usize,
    pub(crate) daily_buckets: usize,
    pub(crate) capped: bool,
}

pub(crate) async fn run_admin_system_cleanup_once(
    data: &GatewayDataState,
) -> Result<AdminSystemCleanupSummary, aether_data::DataLayerError> {
    let audit_logs_deleted = cleanup_audit_logs_once(data).await?;
    let request_candidates_deleted = cleanup_request_candidates_once(data).await?;
    let proxy_node_metrics = cleanup_proxy_node_metrics_once(data).await?;
    let pending = cleanup_stale_pending_requests_once(data).await?;
    let usage = perform_usage_cleanup_once(data).await?;

    Ok(AdminSystemCleanupSummary {
        audit_logs_deleted,
        request_candidates_deleted,
        proxy_node_metrics,
        pending_failed: pending.failed,
        pending_recovered: pending.recovered,
        usage,
    })
}

pub(crate) async fn rebuild_admin_stats_once(
    data: &GatewayDataState,
) -> Result<AdminStatsRebuildSummary, aether_data::DataLayerError> {
    let now_utc = chrono::Utc::now();
    let mut summary = AdminStatsRebuildSummary::default();

    if data.has_stats_hourly_aggregation_backend() {
        let input = aether_data::StatsHourlyAggregationInput {
            target_hour_utc: stats_hourly_aggregation_target_hour(now_utc),
            aggregated_at: now_utc,
        };
        while data.aggregate_stats_hourly(&input).await?.is_some() {
            summary.hourly_buckets = summary.hourly_buckets.saturating_add(1);
            if summary.hourly_buckets >= MAX_ADMIN_STATS_REBUILD_BUCKETS {
                summary.capped = true;
                break;
            }
        }
    }

    if data.has_stats_daily_aggregation_backend() {
        let input = aether_data::StatsDailyAggregationInput {
            target_day_utc: stats_aggregation_target_day(now_utc),
            aggregated_at: now_utc,
        };
        while data.aggregate_stats_daily(&input).await?.is_some() {
            summary.daily_buckets = summary.daily_buckets.saturating_add(1);
            if summary.daily_buckets >= MAX_ADMIN_STATS_REBUILD_BUCKETS {
                summary.capped = true;
                break;
            }
        }
    }

    Ok(summary)
}

pub(crate) async fn cleanup_expired_gemini_file_mappings_once(
    data: &GatewayDataState,
) -> Result<usize, aether_data::DataLayerError> {
    data.delete_expired_gemini_file_mappings(now_unix_secs())
        .await
}

fn summarize_database_pool(data: &GatewayDataState) -> Option<aether_data::DatabasePoolSummary> {
    data.database_pool_summary()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
