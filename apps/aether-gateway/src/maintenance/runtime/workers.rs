use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tracing::{debug, warn};

use crate::data::GatewayDataState;
use crate::AppState;

use super::{
    cleanup_processed_usage_counter_deltas_once, duration_until_next_daily_run,
    duration_until_next_db_maintenance_run, duration_until_next_stats_aggregation_run,
    duration_until_next_stats_hourly_aggregation_run, maintenance_timezone, parse_hhmm_time,
    perform_oauth_token_refresh_once, perform_provider_quota_alert_once, provider_checkin_schedule,
    run_audit_cleanup_once, run_db_maintenance_once, run_gemini_file_mapping_cleanup_once,
    run_pending_cleanup_once, run_pool_monitor_once, run_provider_checkin_once,
    run_proxy_node_metrics_cleanup_once, run_proxy_node_stale_cleanup_once,
    run_proxy_upgrade_rollout_once, run_request_candidate_cleanup_once, run_stats_aggregation_once,
    run_stats_hourly_aggregation_once, run_usage_cleanup_once, run_usage_counter_flush_once,
    run_wallet_daily_usage_aggregation_once, AUDIT_LOG_CLEANUP_INTERVAL,
    GEMINI_FILE_MAPPING_CLEANUP_INTERVAL, OAUTH_TOKEN_REFRESH_INTERVAL, PENDING_CLEANUP_INTERVAL,
    POOL_MONITOR_INTERVAL, PROVIDER_CHECKIN_DEFAULT_TIME, PROVIDER_QUOTA_ALERT_INTERVAL,
    PROXY_NODE_METRICS_CLEANUP_HOUR, PROXY_NODE_METRICS_CLEANUP_MINUTE,
    PROXY_NODE_STALE_SWEEP_INTERVAL, PROXY_UPGRADE_ROLLOUT_INTERVAL,
    REQUEST_CANDIDATE_CLEANUP_INTERVAL, USAGE_CLEANUP_HOUR, USAGE_CLEANUP_MINUTE,
    WALLET_DAILY_USAGE_AGGREGATION_HOUR, WALLET_DAILY_USAGE_AGGREGATION_MINUTE,
};
use super::{UsageCounterFlushRuntimeMetrics, UsageCounterFlushWorkerConfig};

const STATS_DAILY_CATCH_UP_BURST_LIMIT: usize = 14;
const STATS_HOURLY_CATCH_UP_BURST_LIMIT: usize = 72;
const STATS_AGGREGATION_STARTUP_GRACE: Duration = Duration::from_secs(15);
const STATS_CATCH_UP_BUCKET_PAUSE: Duration = Duration::from_secs(10);
const MAINTENANCE_PRESSURE_RETRY_INTERVAL: Duration = Duration::from_secs(30);
static STATS_AGGREGATION_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn log_maintenance_worker_failure(
    worker: &'static str,
    phase: &'static str,
    error: &impl std::fmt::Debug,
) {
    warn!(
        event_name = "maintenance_worker_failed",
        log_type = "ops",
        worker,
        phase,
        error = ?error,
        "gateway maintenance worker failed"
    );
}

fn should_defer_for_database_pressure(
    data: &GatewayDataState,
    worker: &'static str,
    deferred_since: &mut Option<Instant>,
) -> bool {
    let Some(summary) = data.database_pool_summary() else {
        *deferred_since = None;
        return false;
    };
    if !GatewayDataState::should_defer_maintenance_for_pool_pressure_state(
        GatewayDataState::database_pool_summary_under_maintenance_pressure(&summary),
        deferred_since,
    ) {
        return false;
    }

    debug!(
        event_name = "maintenance_worker_deferred",
        log_type = "ops",
        worker,
        driver = %summary.driver,
        checked_out = summary.checked_out,
        pool_size = summary.pool_size,
        idle = summary.idle,
        idle_reserve = GatewayDataState::maintenance_pool_idle_reserve(&summary),
        max_connections = summary.max_connections,
        usage_rate = summary.usage_rate,
        "gateway maintenance worker deferred because database pool has no idle reserve"
    );
    true
}

fn should_defer_stats_aggregation(
    app: &AppState,
    data: &GatewayDataState,
    worker: &'static str,
    deferred_since: &mut Option<Instant>,
) -> bool {
    let database_pool_pressure = data.database_pool_summary().is_some_and(|summary| {
        GatewayDataState::database_pool_summary_under_maintenance_pressure(&summary)
    });
    let foreground_requests_in_flight = app
        .request_concurrency_snapshot()
        .map(|snapshot| snapshot.in_flight)
        .unwrap_or(0);
    if !GatewayDataState::should_defer_maintenance_for_pool_pressure_state(
        database_pool_pressure || foreground_requests_in_flight > 0,
        deferred_since,
    ) {
        return false;
    }

    debug!(
        event_name = "maintenance_worker_deferred",
        log_type = "ops",
        worker,
        database_pool_pressure,
        foreground_requests_in_flight,
        "gateway stats aggregation deferred for foreground traffic"
    );
    true
}

pub(crate) fn spawn_audit_cleanup_worker(app: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_audit_log_reader() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_AUDIT_CLEANUP,
        |app| async move {
            let data = app.data;
            if let Err(err) = run_audit_cleanup_once(&data).await {
                log_maintenance_worker_failure("audit_cleanup", "startup", &err);
            }
            let mut interval = tokio::time::interval(AUDIT_LOG_CLEANUP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(&data, "audit_cleanup", &mut deferred_since) {
                    continue;
                }
                if let Err(err) = run_audit_cleanup_once(&data).await {
                    log_maintenance_worker_failure("audit_cleanup", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_db_maintenance_worker(app: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_database_maintenance_backend() {
        return None;
    }

    let timezone = maintenance_timezone();
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_DB_MAINTENANCE,
        move |app| async move {
            let data = app.data;
            let mut deferred_since = None;
            loop {
                tokio::time::sleep(duration_until_next_db_maintenance_run(Utc::now(), timezone))
                    .await;
                loop {
                    if should_defer_for_database_pressure(
                        &data,
                        "db_maintenance",
                        &mut deferred_since,
                    ) {
                        tokio::time::sleep(MAINTENANCE_PRESSURE_RETRY_INTERVAL).await;
                        continue;
                    }
                    break;
                }
                if let Err(err) = run_db_maintenance_once(&data).await {
                    log_maintenance_worker_failure("db_maintenance", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_wallet_daily_usage_aggregation_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_wallet_daily_usage_aggregation_backend() {
        return None;
    }

    let timezone = maintenance_timezone();
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_WALLET_DAILY_USAGE_AGG,
        move |app| async move {
            let data = app.data;
            let mut deferred_since = None;
            loop {
                tokio::time::sleep(duration_until_next_daily_run(
                    Utc::now(),
                    timezone,
                    WALLET_DAILY_USAGE_AGGREGATION_HOUR,
                    WALLET_DAILY_USAGE_AGGREGATION_MINUTE,
                ))
                .await;
                loop {
                    if should_defer_for_database_pressure(
                        &data,
                        "wallet_daily_usage_aggregation",
                        &mut deferred_since,
                    ) {
                        tokio::time::sleep(MAINTENANCE_PRESSURE_RETRY_INTERVAL).await;
                        continue;
                    }
                    break;
                }
                if let Err(err) = run_wallet_daily_usage_aggregation_once(&data).await {
                    log_maintenance_worker_failure("wallet_daily_usage_aggregation", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_stats_aggregation_worker(app: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_stats_daily_aggregation_backend() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_STATS_DAILY_AGG,
        |app| async move {
            let data = app.data.clone();
            let mut deferred_since = None;
            // Let cold-start authentication and the first navigation finish before catch-up work
            // starts competing for database CPU and I/O.
            tokio::time::sleep(STATS_AGGREGATION_STARTUP_GRACE).await;
            loop {
                let mut processed = 0_usize;
                let mut deferred = false;
                while processed < STATS_DAILY_CATCH_UP_BURST_LIMIT {
                    let permit = STATS_AGGREGATION_GATE
                        .acquire()
                        .await
                        .expect("stats aggregation gate should remain open");
                    if should_defer_stats_aggregation(
                        &app,
                        &data,
                        "stats_daily_aggregation",
                        &mut deferred_since,
                    ) {
                        drop(permit);
                        deferred = true;
                        break;
                    }
                    match run_stats_aggregation_once(&data).await {
                        Ok(true) => {
                            processed += 1;
                            tokio::time::sleep(STATS_CATCH_UP_BUCKET_PAUSE).await;
                            drop(permit);
                        }
                        Ok(false) => break,
                        Err(err) => {
                            log_maintenance_worker_failure("stats_daily_aggregation", "tick", &err);
                            break;
                        }
                    }
                }

                if deferred {
                    tokio::time::sleep(MAINTENANCE_PRESSURE_RETRY_INTERVAL).await;
                    continue;
                }

                if processed >= STATS_DAILY_CATCH_UP_BURST_LIMIT {
                    continue;
                }

                tokio::time::sleep(duration_until_next_stats_aggregation_run(Utc::now())).await;
            }
        },
    ))
}

pub(crate) fn spawn_usage_cleanup_worker(app: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_usage_writer() {
        return None;
    }

    let timezone = maintenance_timezone();
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_USAGE_CLEANUP,
        move |app| async move {
            let data = app.data;
            let mut deferred_since = None;
            loop {
                tokio::time::sleep(duration_until_next_daily_run(
                    Utc::now(),
                    timezone,
                    USAGE_CLEANUP_HOUR,
                    USAGE_CLEANUP_MINUTE,
                ))
                .await;
                loop {
                    if should_defer_for_database_pressure(
                        &data,
                        "usage_cleanup",
                        &mut deferred_since,
                    ) {
                        tokio::time::sleep(MAINTENANCE_PRESSURE_RETRY_INTERVAL).await;
                        continue;
                    }
                    break;
                }
                if let Err(err) = run_usage_cleanup_once(&data).await {
                    log_maintenance_worker_failure("usage_cleanup", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_usage_counter_flush_worker(
    app: AppState,
    metrics: Arc<UsageCounterFlushRuntimeMetrics>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_usage_counter_flush_backend() {
        return None;
    }

    let config = UsageCounterFlushWorkerConfig::from_env();
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_USAGE_COUNTER_FLUSH,
        move |app| {
            let metrics = metrics.clone();
            let config = config.clone();
            async move {
                run_usage_counter_flush_worker_loop(app.data, metrics, config).await;
            }
        },
    ))
}

pub(crate) fn spawn_usage_counter_flush_worker_with_config(
    data: Arc<GatewayDataState>,
    metrics: Arc<UsageCounterFlushRuntimeMetrics>,
    config: UsageCounterFlushWorkerConfig,
) -> Option<tokio::task::JoinHandle<()>> {
    if !data.has_usage_counter_flush_backend() {
        return None;
    }

    Some(tokio::spawn(run_usage_counter_flush_worker_loop(
        data, metrics, config,
    )))
}

async fn run_usage_counter_flush_worker_loop(
    data: Arc<GatewayDataState>,
    metrics: Arc<UsageCounterFlushRuntimeMetrics>,
    config: UsageCounterFlushWorkerConfig,
) {
    let mut interval = tokio::time::interval(config.flush_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    let mut last_delta_cleanup = tokio::time::Instant::now();
    let mut usage_counter_flush_deferred_since = None;
    let mut usage_counter_delta_cleanup_deferred_since = None;

    loop {
        if should_defer_for_database_pressure(
            &data,
            "usage_counter_flush",
            &mut usage_counter_flush_deferred_since,
        ) {
            metrics.record_flush_deferred();
            interval.tick().await;
            continue;
        }

        let mut batches = 0_usize;
        while batches < config.flush_catch_up_burst_limit {
            match run_usage_counter_flush_once(&data, config.flush_batch_size).await {
                Ok(summary) if summary.rows_claimed > 0 => {
                    metrics.record_flush_success(&summary);
                    batches += 1;
                }
                Ok(summary) => {
                    metrics.record_flush_success(&summary);
                    break;
                }
                Err(err) => {
                    metrics.record_flush_failed();
                    log_maintenance_worker_failure("usage_counter_flush", "tick", &err);
                    break;
                }
            }
        }

        if batches >= config.flush_catch_up_burst_limit {
            tokio::task::yield_now().await;
            continue;
        }

        if last_delta_cleanup.elapsed() >= config.cleanup_interval {
            if should_defer_for_database_pressure(
                &data,
                "usage_counter_delta_cleanup",
                &mut usage_counter_delta_cleanup_deferred_since,
            ) {
                metrics.record_cleanup_deferred();
                debug!(
                    event_name = "maintenance_worker_deferred",
                    log_type = "ops",
                    worker = "usage_counter_delta_cleanup",
                    "gateway maintenance worker deferred cleanup under database pressure"
                );
            } else {
                match cleanup_processed_usage_counter_deltas_once(
                    &data,
                    config.delta_retention_secs,
                    config.cleanup_batch_size,
                )
                .await
                {
                    Ok(rows_deleted) => metrics.record_cleanup_success(rows_deleted),
                    Err(err) => {
                        metrics.record_cleanup_failed();
                        log_maintenance_worker_failure("usage_counter_delta_cleanup", "tick", &err);
                    }
                }
            }
            last_delta_cleanup = tokio::time::Instant::now();
        }

        interval.tick().await;
    }
}

pub(crate) fn spawn_provider_checkin_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let timezone = maintenance_timezone();
    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_PROVIDER_CHECKIN,
        move |state| async move {
            let mut deferred_since = None;
            loop {
                let (hour, minute) = match provider_checkin_schedule(&state.data).await {
                    Ok(schedule) => schedule,
                    Err(err) => {
                        warn!(
                            event_name = "maintenance_schedule_lookup_failed",
                            log_type = "ops",
                            worker = "provider_checkin",
                            phase = "schedule_lookup",
                            error = %err,
                            fallback = PROVIDER_CHECKIN_DEFAULT_TIME,
                            "gateway provider checkin schedule lookup failed; falling back"
                        );
                        parse_hhmm_time(PROVIDER_CHECKIN_DEFAULT_TIME)
                            .expect("default provider checkin time should parse")
                    }
                };
                tokio::time::sleep(duration_until_next_daily_run(
                    Utc::now(),
                    timezone,
                    hour,
                    minute,
                ))
                .await;
                loop {
                    if should_defer_for_database_pressure(
                        &state.data,
                        "provider_checkin",
                        &mut deferred_since,
                    ) {
                        tokio::time::sleep(MAINTENANCE_PRESSURE_RETRY_INTERVAL).await;
                        continue;
                    }
                    break;
                }
                if let Err(err) = run_provider_checkin_once(&state).await {
                    log_maintenance_worker_failure("provider_checkin", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_provider_quota_alert_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_PROVIDER_QUOTA_ALERT,
        |state| async move {
            let mut interval = tokio::time::interval(PROVIDER_QUOTA_ALERT_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(
                    &state.data,
                    "provider_quota_alert",
                    &mut deferred_since,
                ) {
                    continue;
                }
                if let Err(err) = perform_provider_quota_alert_once(&state).await {
                    log_maintenance_worker_failure("provider_quota_alert", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_oauth_token_refresh_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_OAUTH_TOKEN_REFRESH,
        |state| async move {
            if let Err(err) = perform_oauth_token_refresh_once(&state).await {
                log_maintenance_worker_failure("oauth_token_refresh", "startup", &err);
            }
            let mut interval = tokio::time::interval(OAUTH_TOKEN_REFRESH_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(
                    &state.data,
                    "oauth_token_refresh",
                    &mut deferred_since,
                ) {
                    continue;
                }
                if let Err(err) = perform_oauth_token_refresh_once(&state).await {
                    log_maintenance_worker_failure("oauth_token_refresh", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_gemini_file_mapping_cleanup_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_gemini_file_mapping_writer() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_GEMINI_FILES_CLEANUP,
        |app| async move {
            let data = app.data;
            if let Err(err) = run_gemini_file_mapping_cleanup_once(&data).await {
                log_maintenance_worker_failure("gemini_file_mapping_cleanup", "startup", &err);
            }
            let mut interval = tokio::time::interval(GEMINI_FILE_MAPPING_CLEANUP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(
                    &data,
                    "gemini_file_mapping_cleanup",
                    &mut deferred_since,
                ) {
                    continue;
                }
                if let Err(err) = run_gemini_file_mapping_cleanup_once(&data).await {
                    log_maintenance_worker_failure("gemini_file_mapping_cleanup", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_pending_cleanup_worker(app: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_usage_writer() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_PENDING_CLEANUP,
        |app| async move {
            let data = app.data;
            if let Err(err) = run_pending_cleanup_once(&data).await {
                log_maintenance_worker_failure("pending_cleanup", "startup", &err);
            }
            let mut interval = tokio::time::interval(PENDING_CLEANUP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(&data, "pending_cleanup", &mut deferred_since)
                {
                    continue;
                }
                if let Err(err) = run_pending_cleanup_once(&data).await {
                    log_maintenance_worker_failure("pending_cleanup", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_proxy_node_stale_cleanup_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_proxy_node_reader() || !app.data.has_proxy_node_writer() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_PROXY_NODE_STALE_CLEANUP,
        |app| async move {
            let data = app.data;
            if let Err(err) = run_proxy_node_stale_cleanup_once(&data).await {
                log_maintenance_worker_failure("proxy_node_stale_cleanup", "startup", &err);
            }
            let mut interval = tokio::time::interval(PROXY_NODE_STALE_SWEEP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(
                    &data,
                    "proxy_node_stale_cleanup",
                    &mut deferred_since,
                ) {
                    continue;
                }
                if let Err(err) = run_proxy_node_stale_cleanup_once(&data).await {
                    log_maintenance_worker_failure("proxy_node_stale_cleanup", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_proxy_node_metrics_cleanup_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_proxy_node_writer() {
        return None;
    }

    let timezone = maintenance_timezone();
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_PROXY_NODE_METRICS_CLEANUP,
        move |app| async move {
            let data = app.data;
            let mut deferred_since = None;
            loop {
                tokio::time::sleep(duration_until_next_daily_run(
                    Utc::now(),
                    timezone,
                    PROXY_NODE_METRICS_CLEANUP_HOUR,
                    PROXY_NODE_METRICS_CLEANUP_MINUTE,
                ))
                .await;
                loop {
                    if should_defer_for_database_pressure(
                        &data,
                        "proxy_node_metrics_cleanup",
                        &mut deferred_since,
                    ) {
                        tokio::time::sleep(MAINTENANCE_PRESSURE_RETRY_INTERVAL).await;
                        continue;
                    }
                    break;
                }
                if let Err(err) = run_proxy_node_metrics_cleanup_once(&data).await {
                    log_maintenance_worker_failure("proxy_node_metrics_cleanup", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_proxy_upgrade_rollout_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.data.has_proxy_node_reader()
        || !state.data.has_proxy_node_writer()
        || !state.data.has_system_config_store()
    {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_PROXY_UPGRADE_ROLLOUT,
        |state| async move {
            if let Err(err) = run_proxy_upgrade_rollout_once(&state).await {
                log_maintenance_worker_failure("proxy_upgrade_rollout", "startup", &err);
            }
            let mut interval = tokio::time::interval(PROXY_UPGRADE_ROLLOUT_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(
                    &state.data,
                    "proxy_upgrade_rollout",
                    &mut deferred_since,
                ) {
                    continue;
                }
                if let Err(err) = run_proxy_upgrade_rollout_once(&state).await {
                    log_maintenance_worker_failure("proxy_upgrade_rollout", "tick", &err);
                }
            }
        },
    ))
}

pub(crate) fn spawn_pool_monitor_worker(app: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_database_pool_summary() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_POOL_MONITOR,
        |app| async move {
            let data = app.data;
            let mut interval = tokio::time::interval(POOL_MONITOR_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                run_pool_monitor_once(&data);
            }
        },
    ))
}

pub(crate) fn spawn_stats_hourly_aggregation_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_stats_hourly_aggregation_backend() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_STATS_HOURLY_AGG,
        |app| async move {
            let data = app.data.clone();
            let mut deferred_since = None;
            tokio::time::sleep(STATS_AGGREGATION_STARTUP_GRACE).await;
            loop {
                let mut processed = 0_usize;
                let mut deferred = false;
                while processed < STATS_HOURLY_CATCH_UP_BURST_LIMIT {
                    let permit = STATS_AGGREGATION_GATE
                        .acquire()
                        .await
                        .expect("stats aggregation gate should remain open");
                    if should_defer_stats_aggregation(
                        &app,
                        &data,
                        "stats_hourly_aggregation",
                        &mut deferred_since,
                    ) {
                        drop(permit);
                        deferred = true;
                        break;
                    }
                    match run_stats_hourly_aggregation_once(&data).await {
                        Ok(true) => {
                            processed += 1;
                            tokio::time::sleep(STATS_CATCH_UP_BUCKET_PAUSE).await;
                            drop(permit);
                        }
                        Ok(false) => break,
                        Err(err) => {
                            log_maintenance_worker_failure(
                                "stats_hourly_aggregation",
                                "tick",
                                &err,
                            );
                            break;
                        }
                    }
                }

                if deferred {
                    tokio::time::sleep(MAINTENANCE_PRESSURE_RETRY_INTERVAL).await;
                    continue;
                }

                if processed >= STATS_HOURLY_CATCH_UP_BURST_LIMIT {
                    continue;
                }

                tokio::time::sleep(duration_until_next_stats_hourly_aggregation_run(Utc::now()))
                    .await;
            }
        },
    ))
}

pub(crate) fn spawn_request_candidate_cleanup_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_request_candidate_writer() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_REQUEST_CANDIDATE_CLEANUP,
        |app| async move {
            let data = app.data;
            if let Err(err) = run_request_candidate_cleanup_once(&data).await {
                log_maintenance_worker_failure("request_candidate_cleanup", "startup", &err);
            }
            let mut interval = tokio::time::interval(REQUEST_CANDIDATE_CLEANUP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if should_defer_for_database_pressure(
                    &data,
                    "request_candidate_cleanup",
                    &mut deferred_since,
                ) {
                    continue;
                }
                if let Err(err) = run_request_candidate_cleanup_once(&data).await {
                    log_maintenance_worker_failure("request_candidate_cleanup", "tick", &err);
                }
            }
        },
    ))
}
