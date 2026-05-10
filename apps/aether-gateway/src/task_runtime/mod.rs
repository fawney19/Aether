use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::background_tasks::{
    BackgroundTaskKind, BackgroundTaskStatus, StoredBackgroundTaskRun, UpsertBackgroundTaskEvent,
    UpsertBackgroundTaskRun,
};
use aether_runtime::task::spawn_named;
pub(crate) use aether_task_runtime::TaskSupervisor;
use aether_task_runtime::{RetryPolicy, TaskDefinition, TaskKind};
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

use crate::{AppState, GatewayError};

pub(crate) const TASK_KEY_PROVIDER_DELETE: &str = "admin.provider.delete";
pub(crate) const TASK_KEY_PROVIDER_OAUTH_BATCH_IMPORT: &str = "admin.provider.oauth.batch_import";
pub(crate) const TASK_KEY_USAGE_QUEUE_WORKER: &str = "usage.queue.worker";
pub(crate) const TASK_KEY_VIDEO_TASK_POLLER: &str = "video.task.poller";
pub(crate) const TASK_KEY_MODEL_FETCH_WORKER: &str = "model.fetch.worker";
pub(crate) const TASK_KEY_PROVIDER_QUOTA_RESET: &str = "provider.quota.reset.worker";
pub(crate) const TASK_KEY_POOL_PREHEAT_PROBE: &str = "pool.preheat.probe.worker";
pub(crate) const TASK_KEY_POOL_QUOTA_PROBE: &str = "pool.quota.probe.worker";
pub(crate) const TASK_KEY_POOL_MONITOR: &str = "pool.monitor.worker";
pub(crate) const TASK_KEY_AUDIT_CLEANUP: &str = "maintenance.audit.cleanup";
pub(crate) const TASK_KEY_DB_MAINTENANCE: &str = "maintenance.database";
pub(crate) const TASK_KEY_PENDING_CLEANUP: &str = "maintenance.pending.cleanup";
pub(crate) const TASK_KEY_REQUEST_CANDIDATE_CLEANUP: &str = "maintenance.request.candidate.cleanup";
pub(crate) const TASK_KEY_GEMINI_FILES_CLEANUP: &str = "maintenance.gemini.files.cleanup";
pub(crate) const TASK_KEY_OAUTH_TOKEN_REFRESH: &str = "maintenance.oauth.token.refresh";
pub(crate) const TASK_KEY_PROXY_NODE_STALE_CLEANUP: &str = "maintenance.proxy.node.stale.cleanup";
pub(crate) const TASK_KEY_PROXY_NODE_METRICS_CLEANUP: &str =
    "maintenance.proxy.node.metrics.cleanup";
pub(crate) const TASK_KEY_PROXY_UPGRADE_ROLLOUT: &str = "maintenance.proxy.upgrade.rollout";
pub(crate) const TASK_KEY_PROVIDER_CHECKIN: &str = "maintenance.provider.checkin";
pub(crate) const TASK_KEY_USAGE_CLEANUP: &str = "maintenance.usage.cleanup";
pub(crate) const TASK_KEY_WALLET_DAILY_USAGE_AGG: &str = "maintenance.wallet.daily.usage.agg";
pub(crate) const TASK_KEY_STATS_DAILY_AGG: &str = "maintenance.stats.daily.agg";
pub(crate) const TASK_KEY_STATS_HOURLY_AGG: &str = "maintenance.stats.hourly.agg";
pub(crate) const TASK_KEY_USAGE_SYNC_REPORT: &str = "usage.sync.report";
pub(crate) const TASK_KEY_PROVIDER_OAUTH_ACCOUNT_REFRESH: &str = "provider.oauth.account.refresh";
pub(crate) const TASK_KEY_PROVIDER_BALANCE_REFRESH: &str = "provider.ops.balance.refresh";

const RETRY_ONCE: RetryPolicy = RetryPolicy { max_attempts: 1 };

const TASK_DEFINITIONS: &[TaskDefinition] = &[
    TaskDefinition::new(
        TASK_KEY_PROVIDER_DELETE,
        TaskKind::OnDemand,
        "manual",
        false,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROVIDER_OAUTH_BATCH_IMPORT,
        TaskKind::OnDemand,
        "manual",
        false,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_USAGE_QUEUE_WORKER,
        TaskKind::Daemon,
        "daemon",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_VIDEO_TASK_POLLER,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_MODEL_FETCH_WORKER,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROVIDER_QUOTA_RESET,
        TaskKind::Scheduled,
        "daily",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_POOL_PREHEAT_PROBE,
        TaskKind::OnDemand,
        "request",
        false,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_POOL_QUOTA_PROBE,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_POOL_MONITOR,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_AUDIT_CLEANUP,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_DB_MAINTENANCE,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PENDING_CLEANUP,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_REQUEST_CANDIDATE_CLEANUP,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_GEMINI_FILES_CLEANUP,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_OAUTH_TOKEN_REFRESH,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROXY_NODE_STALE_CLEANUP,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROXY_NODE_METRICS_CLEANUP,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROXY_UPGRADE_ROLLOUT,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROVIDER_CHECKIN,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_USAGE_CLEANUP,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_WALLET_DAILY_USAGE_AGG,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_STATS_DAILY_AGG,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_STATS_HOURLY_AGG,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_USAGE_SYNC_REPORT,
        TaskKind::FireAndForget,
        "internal",
        false,
        false,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROVIDER_OAUTH_ACCOUNT_REFRESH,
        TaskKind::FireAndForget,
        "internal",
        false,
        false,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROVIDER_BALANCE_REFRESH,
        TaskKind::FireAndForget,
        "internal",
        false,
        false,
        RETRY_ONCE,
    ),
];

pub(crate) fn task_definitions() -> &'static [TaskDefinition] {
    TASK_DEFINITIONS
}

pub(crate) fn task_definition(task_key: &str) -> Option<TaskDefinition> {
    task_definitions()
        .iter()
        .copied()
        .find(|definition| definition.key == task_key)
}

pub(crate) const fn background_task_kind(kind: TaskKind) -> BackgroundTaskKind {
    match kind {
        TaskKind::Scheduled => BackgroundTaskKind::Scheduled,
        TaskKind::Daemon => BackgroundTaskKind::Daemon,
        TaskKind::OnDemand => BackgroundTaskKind::OnDemand,
        TaskKind::FireAndForget => BackgroundTaskKind::FireAndForget,
    }
}

pub(crate) fn task_cancel_kv_key(run_id: &str) -> String {
    format!("task_runtime:run:{run_id}:cancel")
}

pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) fn build_task_run_id() -> String {
    Uuid::new_v4().to_string()
}

pub(crate) fn spawn_fire_and_forget<F>(task_name: &'static str, future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    spawn_named(task_name, future)
}

fn stored_run_to_upsert(run: StoredBackgroundTaskRun) -> UpsertBackgroundTaskRun {
    UpsertBackgroundTaskRun {
        id: run.id,
        task_key: run.task_key,
        kind: run.kind,
        trigger: run.trigger,
        status: run.status,
        attempt: run.attempt,
        max_attempts: run.max_attempts,
        owner_instance: run.owner_instance,
        progress_percent: run.progress_percent,
        progress_message: run.progress_message,
        payload_json: run.payload_json,
        result_json: run.result_json,
        error_message: run.error_message,
        cancel_requested: run.cancel_requested,
        created_by: run.created_by,
        created_at_unix_secs: run.created_at_unix_secs,
        started_at_unix_secs: run.started_at_unix_secs,
        finished_at_unix_secs: run.finished_at_unix_secs,
        updated_at_unix_secs: run.updated_at_unix_secs,
    }
}

pub(crate) async fn upsert_run_with_logging(
    app: &AppState,
    run: UpsertBackgroundTaskRun,
) -> Option<StoredBackgroundTaskRun> {
    match app.upsert_background_task_run(run).await {
        Ok(result) => result,
        Err(error) => {
            warn!(error = ?error, "failed to upsert background task run");
            None
        }
    }
}

pub(crate) async fn update_run_status(
    app: &AppState,
    run_id: &str,
    status: BackgroundTaskStatus,
    progress_percent: Option<u16>,
    progress_message: Option<String>,
    result_json: Option<Value>,
    error_message: Option<String>,
    started_at_unix_secs: Option<u64>,
    finished_at_unix_secs: Option<u64>,
) -> Option<StoredBackgroundTaskRun> {
    let Some(mut existing) = app.find_background_task_run(run_id).await.ok().flatten() else {
        return None;
    };
    existing.status = status;
    if let Some(progress_percent) = progress_percent {
        existing.progress_percent = progress_percent.min(100);
    }
    if let Some(progress_message) = progress_message {
        existing.progress_message = Some(progress_message);
    }
    if result_json.is_some() {
        existing.result_json = result_json;
    }
    if error_message.is_some() {
        existing.error_message = error_message;
    }
    if started_at_unix_secs.is_some() {
        existing.started_at_unix_secs = started_at_unix_secs;
    }
    if finished_at_unix_secs.is_some() {
        existing.finished_at_unix_secs = finished_at_unix_secs;
    }
    existing.updated_at_unix_secs = now_unix_secs();
    upsert_run_with_logging(app, stored_run_to_upsert(existing)).await
}

pub(crate) async fn append_event_with_logging(
    app: &AppState,
    run_id: &str,
    event_type: &str,
    message: &str,
    payload_json: Option<Value>,
) {
    let event = UpsertBackgroundTaskEvent {
        id: Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        event_type: event_type.to_string(),
        message: message.to_string(),
        payload_json,
        created_at_unix_secs: now_unix_secs(),
    };
    if let Err(error) = app.upsert_background_task_event(event).await {
        warn!(error = ?error, run_id = %run_id, "failed to upsert background task event");
    }
}

pub(crate) fn spawn_record_worker_boot(
    app: AppState,
    task_key: &'static str,
    kind: BackgroundTaskKind,
    trigger: &'static str,
) -> JoinHandle<()> {
    spawn_named("task-runtime-record-worker-boot", async move {
        let now = now_unix_secs();
        let run_id = format!("boot:{}:{}", task_key, app.tunnel.local_instance_id());
        let run = UpsertBackgroundTaskRun {
            id: run_id.clone(),
            task_key: task_key.to_string(),
            kind,
            trigger: trigger.to_string(),
            status: BackgroundTaskStatus::Running,
            attempt: 1,
            max_attempts: 1,
            owner_instance: Some(app.tunnel.local_instance_id().to_string()),
            progress_percent: 0,
            progress_message: Some("worker booted".to_string()),
            payload_json: None,
            result_json: None,
            error_message: None,
            cancel_requested: false,
            created_by: Some("system".to_string()),
            created_at_unix_secs: now,
            started_at_unix_secs: Some(now),
            finished_at_unix_secs: None,
            updated_at_unix_secs: now,
        };
        let _ = upsert_run_with_logging(&app, run).await;
        append_event_with_logging(
            &app,
            &run_id,
            "worker_boot",
            "background worker started",
            None,
        )
        .await;
    })
}

pub(crate) async fn set_cancel_signal(app: &AppState, run_id: &str) -> Result<(), GatewayError> {
    app.runtime_kv_setex(&task_cancel_kv_key(run_id), "1", 60 * 60)
        .await
}

pub(crate) async fn is_cancel_requested(app: &AppState, run_id: &str) -> bool {
    if let Ok(Some(run)) = app.find_background_task_run(run_id).await {
        if run.cancel_requested {
            return true;
        }
    }
    app.runtime_kv_exists(&task_cancel_kv_key(run_id))
        .await
        .unwrap_or(false)
}

pub(crate) async fn submit_provider_delete_task(
    state: &crate::admin_api::AdminAppState<'_>,
    provider_id: &str,
    created_by: Option<&str>,
) -> Result<Option<String>, GatewayError> {
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };

    let task_id = Uuid::new_v4().simple().to_string()[..16].to_string();
    state.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
        task_id: task_id.clone(),
        provider_id: provider.id.clone(),
        status: "pending".to_string(),
        stage: "queued".to_string(),
        total_keys: 0,
        deleted_keys: 0,
        total_endpoints: 0,
        deleted_endpoints: 0,
        message: "delete task submitted".to_string(),
    });

    let app = state.cloned_app();
    let provider_id = provider.id.clone();
    let run_id = task_id.clone();
    let created_at = now_unix_secs();
    let max_attempts = task_definition(TASK_KEY_PROVIDER_DELETE)
        .map(|item| item.retry_policy.max_attempts)
        .unwrap_or(1);
    if state.has_background_task_data_writer() {
        let run = UpsertBackgroundTaskRun {
            id: run_id.clone(),
            task_key: TASK_KEY_PROVIDER_DELETE.to_string(),
            kind: BackgroundTaskKind::OnDemand,
            trigger: "manual".to_string(),
            status: BackgroundTaskStatus::Queued,
            attempt: 1,
            max_attempts,
            owner_instance: Some(app.tunnel.local_instance_id().to_string()),
            progress_percent: 0,
            progress_message: Some("delete task queued".to_string()),
            payload_json: Some(serde_json::json!({ "provider_id": provider_id.clone() })),
            result_json: None,
            error_message: None,
            cancel_requested: false,
            created_by: Some(created_by.unwrap_or("admin").to_string()),
            created_at_unix_secs: created_at,
            started_at_unix_secs: None,
            finished_at_unix_secs: None,
            updated_at_unix_secs: created_at,
        };
        let _ = upsert_run_with_logging(&app, run).await;
        append_event_with_logging(
            &app,
            &run_id,
            "queued",
            "provider delete task queued",
            Some(serde_json::json!({ "provider_id": provider_id.clone() })),
        )
        .await;
    }

    spawn_named("task-runtime-provider-delete", async move {
        let lock_key = format!("task_runtime:lock:{TASK_KEY_PROVIDER_DELETE}:{provider_id}");
        let lock_ttl = std::time::Duration::from_secs(60 * 15);
        let lock = app
            .runtime_state
            .lock_try_acquire(&lock_key, app.tunnel.local_instance_id(), lock_ttl)
            .await
            .ok()
            .flatten();
        if lock.is_none() {
            let _ = update_run_status(
                &app,
                &run_id,
                BackgroundTaskStatus::Skipped,
                Some(0),
                Some("provider delete skipped: another node is running this task".to_string()),
                None,
                None,
                None,
                Some(now_unix_secs()),
            )
            .await;
            append_event_with_logging(
                &app,
                &run_id,
                "skipped",
                "provider delete skipped by singleton lock",
                None,
            )
            .await;
            return;
        }

        let started_at = now_unix_secs();
        let _ = update_run_status(
            &app,
            &run_id,
            BackgroundTaskStatus::Running,
            Some(5),
            Some("provider delete task started".to_string()),
            None,
            None,
            Some(started_at),
            None,
        )
        .await;
        append_event_with_logging(
            &app,
            &run_id,
            "running",
            "provider delete task started",
            None,
        )
        .await;

        let admin_state = crate::admin_api::AdminAppState::new(&app);
        let result = admin_state
            .run_admin_provider_delete_task(&provider_id, &run_id)
            .await;
        match result {
            Ok(task_state) => {
                let _ = update_run_status(
                    &app,
                    &run_id,
                    BackgroundTaskStatus::Succeeded,
                    Some(100),
                    Some(task_state.message.clone()),
                    Some(serde_json::json!({
                        "provider_id": task_state.provider_id,
                        "status": task_state.status,
                        "stage": task_state.stage,
                        "deleted_keys": task_state.deleted_keys,
                        "total_keys": task_state.total_keys,
                        "deleted_endpoints": task_state.deleted_endpoints,
                        "total_endpoints": task_state.total_endpoints,
                        "message": task_state.message,
                    })),
                    None,
                    None,
                    Some(now_unix_secs()),
                )
                .await;
                append_event_with_logging(
                    &app,
                    &run_id,
                    "succeeded",
                    "provider delete task completed",
                    None,
                )
                .await;
            }
            Err(err) => {
                warn!(
                    "gateway admin provider delete task failed for provider {}: {:?}",
                    provider_id, err
                );
                app.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
                    task_id: run_id.clone(),
                    provider_id: provider_id.clone(),
                    status: "failed".to_string(),
                    stage: "failed".to_string(),
                    total_keys: 0,
                    deleted_keys: 0,
                    total_endpoints: 0,
                    deleted_endpoints: 0,
                    message: format!("provider delete failed: {err:?}"),
                });
                let _ = update_run_status(
                    &app,
                    &run_id,
                    BackgroundTaskStatus::Failed,
                    Some(100),
                    Some("provider delete task failed".to_string()),
                    None,
                    Some(format!("{err:?}")),
                    None,
                    Some(now_unix_secs()),
                )
                .await;
                append_event_with_logging(
                    &app,
                    &run_id,
                    "failed",
                    "provider delete task failed",
                    Some(serde_json::json!({ "error": format!("{err:?}") })),
                )
                .await;
            }
        }

        if let Some(lock) = lock {
            let _ = app.runtime_state.lock_release(&lock).await;
        }
    });

    Ok(Some(task_id))
}

pub(crate) async fn submit_pool_preheat_probe_task(
    app: &AppState,
    provider_id: &str,
    pool_group_id: &str,
    top_n_key_ids: Vec<String>,
) -> Result<Option<String>, GatewayError> {
    let request = crate::maintenance::PoolPreheatProbeTaskRequest::new(
        provider_id,
        pool_group_id,
        top_n_key_ids,
    );
    if request.provider_id.is_empty() || request.pool_group_id.is_empty() {
        return Ok(None);
    }

    let Some(provider) = app
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&request.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    if !crate::maintenance::pool_preheat_probe_provider_is_supported(&provider.provider_type) {
        return Ok(None);
    }

    let run_id = build_task_run_id();
    let created_at = now_unix_secs();
    let max_attempts = task_definition(TASK_KEY_POOL_PREHEAT_PROBE)
        .map(|item| item.retry_policy.max_attempts)
        .unwrap_or(1);
    if app.has_background_task_data_writer() {
        let run = UpsertBackgroundTaskRun {
            id: run_id.clone(),
            task_key: TASK_KEY_POOL_PREHEAT_PROBE.to_string(),
            kind: BackgroundTaskKind::OnDemand,
            trigger: "request".to_string(),
            status: BackgroundTaskStatus::Queued,
            attempt: 1,
            max_attempts,
            owner_instance: Some(app.tunnel.local_instance_id().to_string()),
            progress_percent: 0,
            progress_message: Some("pool preheat probe queued".to_string()),
            payload_json: Some(serde_json::json!({
                "provider_id": request.provider_id.clone(),
                "pool_group_id": request.pool_group_id.clone(),
                "top_n_key_ids": request.top_n_key_ids.clone(),
            })),
            result_json: None,
            error_message: None,
            cancel_requested: false,
            created_by: Some("request".to_string()),
            created_at_unix_secs: created_at,
            started_at_unix_secs: None,
            finished_at_unix_secs: None,
            updated_at_unix_secs: created_at,
        };
        let _ = upsert_run_with_logging(app, run).await;
        append_event_with_logging(
            app,
            &run_id,
            "queued",
            "pool preheat probe queued",
            Some(serde_json::json!({
                "provider_id": request.provider_id.clone(),
                "pool_group_id": request.pool_group_id.clone(),
                "top_n_key_ids": request.top_n_key_ids.clone(),
            })),
        )
        .await;
    }

    let task_app = app.clone();
    let task_request = request.clone();
    let task_run_id = run_id.clone();
    spawn_named("task-runtime-pool-preheat-probe", async move {
        let lock_key = format!(
            "task_runtime:lock:{TASK_KEY_POOL_PREHEAT_PROBE}:{}:{}",
            task_request.provider_id, task_request.pool_group_id
        );
        let lock_ttl = std::time::Duration::from_secs(60 * 2);
        let lock = task_app
            .runtime_state
            .lock_try_acquire(&lock_key, task_app.tunnel.local_instance_id(), lock_ttl)
            .await
            .ok()
            .flatten();
        if lock.is_none() {
            let _ = update_run_status(
                &task_app,
                &task_run_id,
                BackgroundTaskStatus::Skipped,
                Some(0),
                Some("pool preheat probe skipped: another node is running this group".to_string()),
                None,
                None,
                None,
                Some(now_unix_secs()),
            )
            .await;
            append_event_with_logging(
                &task_app,
                &task_run_id,
                "skipped",
                "pool preheat probe skipped by group lock",
                None,
            )
            .await;
            return;
        }

        let started_at = now_unix_secs();
        let _ = update_run_status(
            &task_app,
            &task_run_id,
            BackgroundTaskStatus::Running,
            Some(5),
            Some("pool preheat probe started".to_string()),
            None,
            None,
            Some(started_at),
            None,
        )
        .await;
        append_event_with_logging(
            &task_app,
            &task_run_id,
            "running",
            "pool preheat probe started",
            None,
        )
        .await;

        match crate::maintenance::perform_pool_preheat_probe(&task_app, task_request.clone()).await
        {
            Ok(summary) => {
                let status = if summary.task_status_is_skipped() {
                    BackgroundTaskStatus::Skipped
                } else {
                    BackgroundTaskStatus::Succeeded
                };
                let progress_message = if summary.task_status_is_skipped() {
                    summary
                        .skipped_reason
                        .clone()
                        .unwrap_or_else(|| "pool preheat probe skipped".to_string())
                } else {
                    format!(
                        "pool preheat probe completed: {} probed, {} healthy",
                        summary.probed_keys, summary.healthy
                    )
                };
                let _ = update_run_status(
                    &task_app,
                    &task_run_id,
                    status,
                    Some(100),
                    Some(progress_message),
                    Some(serde_json::to_value(&summary).unwrap_or_else(|_| serde_json::json!({}))),
                    None,
                    None,
                    Some(now_unix_secs()),
                )
                .await;
                append_event_with_logging(
                    &task_app,
                    &task_run_id,
                    if status == BackgroundTaskStatus::Skipped {
                        "skipped"
                    } else {
                        "succeeded"
                    },
                    if status == BackgroundTaskStatus::Skipped {
                        "pool preheat probe skipped"
                    } else {
                        "pool preheat probe completed"
                    },
                    None,
                )
                .await;
            }
            Err(err) => {
                warn!(
                    run_id = %task_run_id,
                    provider_id = %task_request.provider_id,
                    error = ?err,
                    "pool preheat probe task failed"
                );
                let _ = update_run_status(
                    &task_app,
                    &task_run_id,
                    BackgroundTaskStatus::Failed,
                    Some(100),
                    Some("pool preheat probe failed".to_string()),
                    None,
                    Some(format!("{err:?}")),
                    None,
                    Some(now_unix_secs()),
                )
                .await;
                append_event_with_logging(
                    &task_app,
                    &task_run_id,
                    "failed",
                    "pool preheat probe failed",
                    Some(serde_json::json!({ "error": format!("{err:?}") })),
                )
                .await;
            }
        }

        if let Some(lock) = lock {
            let _ = task_app.runtime_state.lock_release(&lock).await;
        }
    });

    Ok(Some(run_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use aether_data::repository::background_tasks::InMemoryBackgroundTaskRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::background_tasks::BackgroundTaskStatus;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
    };
    use aether_task_runtime::TaskKind;

    fn provider(provider_type: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-preheat".to_string(),
            "preheat".to_string(),
            Some("https://example.com".to_string()),
            provider_type.to_string(),
        )
        .expect("provider should build")
    }

    fn endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-preheat".to_string(),
            "provider-preheat".to_string(),
            "openai:responses".to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://example.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn state_with_task_repository(
        provider_catalog_repository: Arc<InMemoryProviderCatalogReadRepository>,
        background_task_repository: Arc<InMemoryBackgroundTaskRepository>,
    ) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_background_task_repository_for_tests(background_task_repository),
            )
    }

    #[test]
    fn pool_preheat_probe_task_definition_is_registered() {
        let definition = task_definition(TASK_KEY_POOL_PREHEAT_PROBE)
            .expect("pool preheat task definition should exist");

        assert_eq!(definition.kind, TaskKind::OnDemand);
        assert_eq!(definition.trigger, "request");
        assert!(!definition.singleton);
        assert!(definition.persist_history);
        assert_eq!(definition.retry_policy.max_attempts, 1);
    }

    #[tokio::test]
    async fn submit_pool_preheat_probe_task_returns_queued_run_id() {
        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider("codex")],
            vec![endpoint()],
            vec![],
        ));
        let background_task_repository = Arc::new(InMemoryBackgroundTaskRepository::default());
        let state =
            state_with_task_repository(provider_catalog_repository, background_task_repository);

        let run_id = submit_pool_preheat_probe_task(
            &state,
            "provider-preheat",
            "provider=provider-preheat|endpoint=endpoint-preheat|model=model-1|selected_model=gpt-5|api_format=openai:responses|singleton_key=*",
            vec!["key-preheat".to_string()],
        )
        .await
        .expect("submit should succeed")
        .expect("run id should be returned");
        let run = state
            .find_background_task_run(&run_id)
            .await
            .expect("run lookup should succeed")
            .expect("queued run should exist");

        assert_eq!(run.id, run_id);
        assert_eq!(run.task_key, TASK_KEY_POOL_PREHEAT_PROBE);
        assert_eq!(run.kind, BackgroundTaskKind::OnDemand);
        assert_eq!(run.trigger, "request");
        assert_eq!(run.status, BackgroundTaskStatus::Queued);
        assert_eq!(
            run.payload_json
                .as_ref()
                .and_then(|value| value.get("provider_id")),
            Some(&serde_json::json!("provider-preheat"))
        );
    }

    #[tokio::test]
    async fn submit_pool_preheat_probe_task_skips_unsupported_provider_without_run() {
        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider("openai")],
            vec![endpoint()],
            vec![],
        ));
        let background_task_repository = Arc::new(InMemoryBackgroundTaskRepository::default());
        let state =
            state_with_task_repository(provider_catalog_repository, background_task_repository);

        let run_id = submit_pool_preheat_probe_task(
            &state,
            "provider-preheat",
            "pool-group",
            vec!["key-preheat".to_string()],
        )
        .await
        .expect("submit should succeed");

        assert!(run_id.is_none());
    }
}
