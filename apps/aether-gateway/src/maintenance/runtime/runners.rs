use aether_data_contracts::DataLayerError;
use tracing::{info, warn};

use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

use super::{
    advance_proxy_upgrade_rollout_once, cleanup_audit_logs_once,
    cleanup_expired_gemini_file_mappings_once, cleanup_request_candidates_once,
    cleanup_stale_pending_requests_once, cleanup_stale_proxy_nodes_once,
    collect_proxy_upgrade_rollout_probes, perform_db_maintenance_once,
    perform_provider_checkin_once, perform_stats_aggregation_once,
    perform_stats_hourly_aggregation_once, perform_usage_cleanup_once,
    perform_wallet_daily_usage_aggregation_once, record_proxy_upgrade_traffic_success,
    summarize_postgres_pool,
};

pub(super) async fn run_audit_cleanup_once(data: &GatewayDataState) -> Result<(), DataLayerError> {
    let deleted = cleanup_audit_logs_once(data).await?;
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

pub(super) async fn run_proxy_upgrade_rollout_once(state: &AppState) -> Result<(), DataLayerError> {
    let mut summary = advance_proxy_upgrade_rollout_once(&state.data).await?;
    let probes = collect_proxy_upgrade_rollout_probes(&state.data).await?;
    let mut probe_recorded = false;
    for probe in probes {
        match state
            .tunnel
            .probe_node_url(&probe.node_id, &probe.url, probe.timeout_secs)
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
) -> Result<(), DataLayerError> {
    let Some(summary) = perform_stats_aggregation_once(data).await? else {
        return Ok(());
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
    Ok(())
}

pub(super) async fn run_usage_cleanup_once(data: &GatewayDataState) -> Result<(), DataLayerError> {
    let summary = perform_usage_cleanup_once(data).await?;
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

pub(super) fn run_pool_monitor_once(data: &GatewayDataState) {
    let Some(summary) = summarize_postgres_pool(data) else {
        return;
    };

    info!(
        event_name = "postgres_pool_sampled",
        log_type = "ops",
        worker = "pool_monitor",
        checked_out = summary.checked_out,
        pool_size = summary.pool_size,
        idle = summary.idle,
        max_connections = summary.max_connections,
        usage_rate = summary.usage_rate,
        "gateway postgres pool status"
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
) -> Result<(), DataLayerError> {
    let Some(summary) = perform_stats_hourly_aggregation_once(data).await? else {
        return Ok(());
    };

    info!(
        event_name = "stats_hourly_aggregation_completed",
        log_type = "ops",
        worker = "stats_hourly_aggregation",
        hour_utc = %summary.hour_utc,
        total_requests = summary.total_requests,
        user_rows = summary.user_rows,
        model_rows = summary.model_rows,
        provider_rows = summary.provider_rows,
        "gateway aggregated stats hourly tables"
    );
    Ok(())
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
