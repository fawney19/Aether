use aether_data_contracts::DataLayerError;
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

use super::{
    advance_proxy_upgrade_rollout_once, cleanup_audit_logs_once,
    cleanup_expired_gemini_file_mappings_once, cleanup_proxy_node_metrics_once,
    cleanup_request_candidates_once, cleanup_stale_pending_requests_once,
    cleanup_stale_proxy_nodes_once, collect_proxy_upgrade_rollout_probes, now_unix_secs,
    perform_db_maintenance_once, perform_manual_usage_cleanup_once, perform_provider_checkin_once,
    perform_stats_aggregation_once, perform_stats_hourly_aggregation_once,
    perform_usage_cleanup_once, perform_wallet_daily_usage_aggregation_once,
    record_admin_cleanup_run, record_completed_cleanup_run, record_failed_cleanup_run,
    record_proxy_upgrade_traffic_success, summarize_database_pool, AdminCleanupRunRecord,
    ManualUsageCleanupOptions,
};

pub(super) async fn run_audit_cleanup_once(data: &GatewayDataState) -> Result<(), DataLayerError> {
    let started_at_unix_secs = now_unix_secs();
    let started_at = Instant::now();
    let deleted = match cleanup_audit_logs_once(data).await {
        Ok(deleted) => deleted,
        Err(err) => {
            record_failed_cleanup_run(
                data,
                "audit_cleanup",
                "auto",
                started_at_unix_secs,
                started_at,
                &err,
            )
            .await;
            return Err(err);
        }
    };
    record_completed_cleanup_run(
        data,
        "audit_cleanup",
        "auto",
        started_at_unix_secs,
        started_at,
        json!({ "audit_logs_deleted": deleted }),
        format!("审计日志自动清理完成，删除 {deleted} 行"),
    )
    .await;
    if deleted > 0 {
        info!(
            event_name = "audit_cleanup_completed",
            log_type = "ops",
            worker = "audit_cleanup",
            deleted,
            "gateway deleted expired audit logs"
        );
    }
    Ok(())
}

pub(super) async fn run_gemini_file_mapping_cleanup_once(
    data: &GatewayDataState,
) -> Result<(), DataLayerError> {
    let deleted = cleanup_expired_gemini_file_mappings_once(data).await?;
    if deleted > 0 {
        info!(
            event_name = "gemini_file_mapping_cleanup_completed",
            log_type = "ops",
            worker = "gemini_file_mapping_cleanup",
            deleted,
            "gateway deleted expired gemini file mappings"
        );
    }
    Ok(())
}

pub(super) async fn run_proxy_node_stale_cleanup_once(
    data: &GatewayDataState,
) -> Result<(), DataLayerError> {
    let stale_marked = cleanup_stale_proxy_nodes_once(data).await?;
    if stale_marked > 0 {
        info!(
            event_name = "proxy_node_stale_cleanup_completed",
            log_type = "ops",
            worker = "proxy_node_stale_cleanup",
            stale_marked,
            "gateway marked stale proxy nodes offline"
        );
    }
    Ok(())
}

pub(super) async fn run_proxy_node_metrics_cleanup_once(
    data: &GatewayDataState,
) -> Result<(), DataLayerError> {
    let summary = cleanup_proxy_node_metrics_once(data).await?;
    if summary.deleted_1m_rows > 0 || summary.deleted_1h_rows > 0 {
        info!(
            event_name = "proxy_node_metrics_cleanup_completed",
            log_type = "ops",
            worker = "proxy_node_metrics_cleanup",
            deleted_1m_rows = summary.deleted_1m_rows,
            deleted_1h_rows = summary.deleted_1h_rows,
            "gateway deleted expired proxy node metrics buckets"
        );
    }
    Ok(())
}

pub(super) async fn run_proxy_upgrade_rollout_once(state: &AppState) -> Result<(), DataLayerError> {
    let mut summary = advance_proxy_upgrade_rollout_once(&state.data).await?;
    let probes = collect_proxy_upgrade_rollout_probes(&state.data).await?;
    let mut probe_recorded = false;
    for probe in probes {
        match state
            .tunnel
            .probe_node_url_routed(state, &probe.node_id, &probe.url, probe.timeout_secs)
            .await
        {
            Ok(status) if (200..300).contains(&status) => {
                let _ = record_proxy_upgrade_traffic_success(&state.data, &probe.node_id).await?;
                probe_recorded = true;
                info!(
                    event_name = "proxy_upgrade_rollout_probe_succeeded",
                    log_type = "ops",
                    worker = "proxy_upgrade_rollout",
                    node_id = %probe.node_id,
                    url = %probe.url,
                    status,
                    "gateway confirmed proxy upgrade health probe"
                );
            }
            Ok(status) => {
                warn!(
                    event_name = "proxy_upgrade_rollout_probe_unhealthy",
                    log_type = "ops",
                    worker = "proxy_upgrade_rollout",
                    node_id = %probe.node_id,
                    url = %probe.url,
                    status,
                    "gateway proxy upgrade health probe returned non-success status"
                );
            }
            Err(error) => {
                warn!(
                    event_name = "proxy_upgrade_rollout_probe_failed",
                    log_type = "ops",
                    worker = "proxy_upgrade_rollout",
                    node_id = %probe.node_id,
                    url = %probe.url,
                    error = %error,
                    "gateway proxy upgrade health probe failed"
                );
            }
        }
    }

    if probe_recorded {
        summary = advance_proxy_upgrade_rollout_once(&state.data).await?;
    }
    if summary.updated > 0 || !summary.pending_node_ids.is_empty() || !summary.version.is_empty() {
        info!(
            event_name = "proxy_upgrade_rollout_checked",
            log_type = "ops",
            worker = "proxy_upgrade_rollout",
            version = %summary.version,
            updated = summary.updated,
            blocked = summary.blocked,
            pending = summary.pending_node_ids.len(),
            completed = summary.completed,
            remaining = summary.remaining,
            rollout_active = summary.rollout_active,
            "gateway checked proxy upgrade rollout"
        );
    }
    Ok(())
}

pub(super) async fn run_db_maintenance_once(data: &GatewayDataState) -> Result<(), DataLayerError> {
    let summary = perform_db_maintenance_once(data).await?;
    if summary.attempted > 0 {
        info!(
            event_name = "db_maintenance_completed",
            log_type = "ops",
            worker = "db_maintenance",
            attempted = summary.attempted,
            succeeded = summary.succeeded,
            failed = summary.attempted.saturating_sub(summary.succeeded),
            "gateway finished db maintenance"
        );
    }
    Ok(())
}

pub(super) async fn run_wallet_daily_usage_aggregation_once(
    data: &GatewayDataState,
) -> Result<(), DataLayerError> {
    let summary = perform_wallet_daily_usage_aggregation_once(data).await?;
    info!(
        event_name = "wallet_daily_usage_aggregation_completed",
        log_type = "ops",
        worker = "wallet_daily_usage_aggregation",
        billing_date = %summary.billing_date,
        billing_timezone = %summary.billing_timezone,
        wallets = summary.aggregated_wallets,
        stale_deleted = summary.deleted_stale_ledgers,
        "gateway aggregated wallet daily usage ledgers"
    );
    Ok(())
}

pub(super) async fn run_stats_aggregation_once(
    data: &GatewayDataState,
) -> Result<bool, DataLayerError> {
    let Some(summary) = perform_stats_aggregation_once(data).await? else {
        return Ok(false);
    };

    info!(
        event_name = "stats_daily_aggregation_completed",
        log_type = "ops",
        worker = "stats_daily_aggregation",
        day_start_utc = %summary.day_start_utc,
        total_requests = summary.total_requests,
        model_rows = summary.model_rows,
        provider_rows = summary.provider_rows,
        api_key_rows = summary.api_key_rows,
        error_rows = summary.error_rows,
        user_rows = summary.user_rows,
        "gateway aggregated daily stats tables"
    );
    Ok(true)
}

pub(super) async fn run_usage_cleanup_once(data: &GatewayDataState) -> Result<(), DataLayerError> {
    let started_at_unix_secs = now_unix_secs();
    let started_at = Instant::now();
    let summary = match perform_usage_cleanup_once(data).await {
        Ok(summary) => summary,
        Err(err) => {
            record_failed_cleanup_run(
                data,
                "usage_cleanup",
                "auto",
                started_at_unix_secs,
                started_at,
                &err,
            )
            .await;
            return Err(err);
        }
    };
    record_completed_cleanup_run(
        data,
        "usage_cleanup",
        "auto",
        started_at_unix_secs,
        started_at,
        json!({
            "body_externalized": summary.body_externalized,
            "legacy_body_refs_migrated": summary.legacy_body_refs_migrated,
            "body_cleaned": summary.body_cleaned,
            "header_cleaned": summary.header_cleaned,
            "keys_cleaned": summary.keys_cleaned,
            "records_deleted": summary.records_deleted,
        }),
        format!(
            "请求记录自动清理完成，影响 {} 项",
            summary
                .body_externalized
                .saturating_add(summary.legacy_body_refs_migrated)
                .saturating_add(summary.body_cleaned)
                .saturating_add(summary.header_cleaned)
                .saturating_add(summary.keys_cleaned)
                .saturating_add(summary.records_deleted)
        ),
    )
    .await;
    if summary.body_externalized > 0
        || summary.legacy_body_refs_migrated > 0
        || summary.body_cleaned > 0
        || summary.header_cleaned > 0
        || summary.keys_cleaned > 0
        || summary.records_deleted > 0
    {
        info!(
            event_name = "usage_cleanup_completed",
            log_type = "ops",
            worker = "usage_cleanup",
            body_externalized = summary.body_externalized,
            legacy_body_refs_migrated = summary.legacy_body_refs_migrated,
            body_cleaned = summary.body_cleaned,
            header_cleaned = summary.header_cleaned,
            keys_cleaned = summary.keys_cleaned,
            records_deleted = summary.records_deleted,
            "gateway finished usage cleanup"
        );
    }
    Ok(())
}

pub(crate) async fn start_manual_usage_cleanup_task(
    data: Arc<GatewayDataState>,
    options: ManualUsageCleanupOptions,
    actor_user_id: Option<String>,
) -> Result<AdminCleanupRunRecord, ManualUsageCleanupError> {
    use super::{list_admin_cleanup_run_records, USAGE_CLEANUP_KIND};

    let existing = list_admin_cleanup_run_records(&data)
        .await
        .map_err(ManualUsageCleanupError::DataLayer)?;
    if existing
        .iter()
        .any(|record| record.kind == USAGE_CLEANUP_KIND && record.status == "processing")
    {
        return Err(ManualUsageCleanupError::AlreadyRunning);
    }

    let started_at_unix_secs = now_unix_secs();
    let record = AdminCleanupRunRecord {
        id: uuid::Uuid::new_v4().to_string(),
        kind: USAGE_CLEANUP_KIND.to_string(),
        trigger: "manual".to_string(),
        status: "processing".to_string(),
        message: manual_usage_cleanup_start_message(options),
        started_at_unix_secs,
        completed_at_unix_secs: None,
        duration_ms: None,
        summary: manual_usage_cleanup_progress_summary(options, 0, None, actor_user_id.as_deref()),
        error: None,
    };
    record_admin_cleanup_run(&data, record.clone())
        .await
        .map_err(ManualUsageCleanupError::DataLayer)?;

    tokio::spawn(run_manual_usage_cleanup_task(
        data,
        record.clone(),
        options,
        actor_user_id,
    ));
    Ok(record)
}

pub(crate) async fn run_manual_usage_cleanup_once(
    data: &GatewayDataState,
    options: ManualUsageCleanupOptions,
    actor_user_id: Option<String>,
) -> Result<aether_data_contracts::repository::usage::UsageCleanupSummary, ManualUsageCleanupError>
{
    use super::USAGE_CLEANUP_KIND;

    let started_at = Instant::now();
    let started_at_unix_secs = now_unix_secs();
    let summary = match perform_manual_usage_cleanup_once(data, options).await {
        Ok(summary) => summary,
        Err(err) => {
            record_failed_cleanup_run(
                data,
                USAGE_CLEANUP_KIND,
                "manual",
                started_at_unix_secs,
                started_at,
                &err,
            )
            .await;
            return Err(ManualUsageCleanupError::DataLayer(err));
        }
    };
    let total = usage_cleanup_total(summary);
    let message = manual_usage_cleanup_completed_message(options, total);
    record_completed_cleanup_run(
        data,
        USAGE_CLEANUP_KIND,
        "manual",
        started_at_unix_secs,
        started_at,
        json!({
            "body_externalized": summary.body_externalized,
            "legacy_body_refs_migrated": summary.legacy_body_refs_migrated,
            "body_cleaned": summary.body_cleaned,
            "header_cleaned": summary.header_cleaned,
            "keys_cleaned": summary.keys_cleaned,
            "records_deleted": summary.records_deleted,
            "mode": options.mode.as_str(),
            "requested_older_than_days": options.requested_older_than_days,
            "targets": options.targets,
            "actor_user_id": actor_user_id,
        }),
        message,
    )
    .await;
    info!(
        event_name = "usage_cleanup_manual_completed",
        log_type = "ops",
        worker = "usage_cleanup",
        trigger = "manual",
        mode = options.mode.as_str(),
        requested_older_than_days = options.requested_older_than_days,
        actor_user_id = actor_user_id.as_deref(),
        total_affected = total,
        "gateway finished manual usage cleanup"
    );
    Ok(summary)
}

async fn run_manual_usage_cleanup_task(
    data: Arc<GatewayDataState>,
    initial_record: AdminCleanupRunRecord,
    options: ManualUsageCleanupOptions,
    actor_user_id: Option<String>,
) {
    let started_at = Instant::now();
    match perform_manual_usage_cleanup_once(&data, options).await {
        Ok(summary) => {
            let total = usage_cleanup_total(summary);
            let record = AdminCleanupRunRecord {
                id: initial_record.id,
                kind: initial_record.kind,
                trigger: initial_record.trigger,
                status: "completed".to_string(),
                message: manual_usage_cleanup_completed_message(options, total),
                started_at_unix_secs: initial_record.started_at_unix_secs,
                completed_at_unix_secs: Some(now_unix_secs()),
                duration_ms: Some(
                    started_at
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX),
                ),
                summary: manual_usage_cleanup_progress_summary(
                    options,
                    100,
                    Some(summary),
                    actor_user_id.as_deref(),
                ),
                error: None,
            };
            if let Err(err) = record_admin_cleanup_run(&data, record).await {
                warn!(error = %err, "failed to record manual usage cleanup completion");
            }
            info!(
                event_name = "usage_cleanup_manual_completed",
                log_type = "ops",
                worker = "usage_cleanup",
                trigger = "manual",
                mode = options.mode.as_str(),
                requested_older_than_days = options.requested_older_than_days,
                actor_user_id = actor_user_id.as_deref(),
                total_affected = total,
                "gateway finished manual usage cleanup task"
            );
        }
        Err(err) => {
            let record = AdminCleanupRunRecord {
                id: initial_record.id,
                kind: initial_record.kind,
                trigger: initial_record.trigger,
                status: "failed".to_string(),
                message: "请求记录手动清理失败".to_string(),
                started_at_unix_secs: initial_record.started_at_unix_secs,
                completed_at_unix_secs: Some(now_unix_secs()),
                duration_ms: Some(
                    started_at
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX),
                ),
                summary: manual_usage_cleanup_progress_summary(
                    options,
                    100,
                    None,
                    actor_user_id.as_deref(),
                ),
                error: Some(err.to_string()),
            };
            if let Err(record_err) = record_admin_cleanup_run(&data, record).await {
                warn!(error = %record_err, "failed to record manual usage cleanup failure");
            }
            warn!(error = %err, "manual usage cleanup task failed");
        }
    }
}

fn usage_cleanup_total(
    summary: aether_data_contracts::repository::usage::UsageCleanupSummary,
) -> usize {
    summary
        .body_externalized
        .saturating_add(summary.legacy_body_refs_migrated)
        .saturating_add(summary.body_cleaned)
        .saturating_add(summary.header_cleaned)
        .saturating_add(summary.keys_cleaned)
        .saturating_add(summary.records_deleted)
}

fn manual_usage_cleanup_start_message(options: ManualUsageCleanupOptions) -> String {
    match options.mode {
        super::ManualUsageCleanupMode::BeforeNow => {
            "请求记录手动清理已开始，清理当前时刻之前的已选请求体".to_string()
        }
        super::ManualUsageCleanupMode::OlderThanDays => format!(
            "请求记录手动清理已开始，清理 {} 天前的已选内容",
            options.requested_older_than_days.unwrap_or_default()
        ),
        super::ManualUsageCleanupMode::Policy => {
            "请求记录手动清理已开始，按当前策略清理已选内容".to_string()
        }
    }
}

fn manual_usage_cleanup_completed_message(
    options: ManualUsageCleanupOptions,
    total: usize,
) -> String {
    match options.mode {
        super::ManualUsageCleanupMode::BeforeNow => {
            format!("请求记录手动清理完成，已清理当前时刻之前的已选请求体，影响 {total} 项")
        }
        super::ManualUsageCleanupMode::OlderThanDays => format!(
            "请求记录手动清理完成，清理 {} 天前的已选内容，影响 {total} 项",
            options.requested_older_than_days.unwrap_or_default()
        ),
        super::ManualUsageCleanupMode::Policy => {
            format!("请求记录手动清理完成（按当前策略），影响 {total} 项")
        }
    }
}

fn manual_usage_cleanup_progress_summary(
    options: ManualUsageCleanupOptions,
    progress_percent: u8,
    summary: Option<aether_data_contracts::repository::usage::UsageCleanupSummary>,
    actor_user_id: Option<&str>,
) -> serde_json::Value {
    let summary = summary.unwrap_or_default();
    json!({
        "mode": options.mode.as_str(),
        "requested_older_than_days": options.requested_older_than_days,
        "targets": options.targets,
        "progress_percent": progress_percent,
        "body_externalized": summary.body_externalized,
        "legacy_body_refs_migrated": summary.legacy_body_refs_migrated,
        "body_cleaned": summary.body_cleaned,
        "header_cleaned": summary.header_cleaned,
        "keys_cleaned": summary.keys_cleaned,
        "records_deleted": summary.records_deleted,
        "total": usage_cleanup_total(summary),
        "actor_user_id": actor_user_id,
    })
}

#[derive(Debug)]
pub(crate) enum ManualUsageCleanupError {
    AlreadyRunning,
    DataLayer(DataLayerError),
}

impl std::fmt::Display for ManualUsageCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("a usage cleanup run is already in progress"),
            Self::DataLayer(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ManualUsageCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AlreadyRunning => None,
            Self::DataLayer(err) => Some(err),
        }
    }
}

pub(super) fn run_pool_monitor_once(data: &GatewayDataState) {
    let Some(summary) = summarize_database_pool(data) else {
        return;
    };

    info!(
        event_name = "database_pool_sampled",
        log_type = "ops",
        worker = "pool_monitor",
        driver = %summary.driver,
        checked_out = summary.checked_out,
        pool_size = summary.pool_size,
        idle = summary.idle,
        max_connections = summary.max_connections,
        usage_rate = summary.usage_rate,
        "gateway database pool status"
    );
}

pub(super) async fn run_pending_cleanup_once(
    data: &GatewayDataState,
) -> Result<(), DataLayerError> {
    let summary = cleanup_stale_pending_requests_once(data).await?;
    if summary.failed > 0 || summary.recovered > 0 {
        info!(
            event_name = "pending_cleanup_completed",
            log_type = "ops",
            worker = "pending_cleanup",
            failed = summary.failed,
            recovered = summary.recovered,
            "gateway cleaned stale pending and streaming requests"
        );
    }
    Ok(())
}

pub(super) async fn run_stats_hourly_aggregation_once(
    data: &GatewayDataState,
) -> Result<bool, DataLayerError> {
    let Some(summary) = perform_stats_hourly_aggregation_once(data).await? else {
        return Ok(false);
    };

    info!(
        event_name = "stats_hourly_aggregation_completed",
        log_type = "ops",
        worker = "stats_hourly_aggregation",
        hour_utc = %summary.hour_utc,
        total_requests = summary.total_requests,
        user_rows = summary.user_rows,
        user_model_rows = summary.user_model_rows,
        model_rows = summary.model_rows,
        provider_rows = summary.provider_rows,
        "gateway aggregated stats hourly tables"
    );
    Ok(true)
}

pub(super) async fn run_provider_checkin_once(state: &AppState) -> Result<(), GatewayError> {
    let summary = perform_provider_checkin_once(state).await?;
    if summary.attempted > 0 {
        info!(
            event_name = "provider_checkin_completed",
            log_type = "ops",
            worker = "provider_checkin",
            attempted = summary.attempted,
            succeeded = summary.succeeded,
            failed = summary.failed,
            skipped = summary.skipped,
            "gateway finished provider checkin"
        );
    }
    Ok(())
}
