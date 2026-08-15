use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::background_tasks::{
    BackgroundTaskKind, BackgroundTaskStatus, StoredBackgroundTaskRun, UpsertBackgroundTaskEvent,
    UpsertBackgroundTaskRun,
};
use aether_runtime::task::spawn_named;
use aether_task_runtime::{RetryPolicy, TaskDefinition, TaskKind};
pub(crate) use aether_task_runtime::{TaskSupervisor, TaskSupervisorMetrics};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

use crate::{AppState, GatewayError};

pub(crate) const TASK_KEY_PROVIDER_DELETE: &str = "admin.provider.delete";
pub(crate) const TASK_KEY_PROVIDER_OAUTH_BATCH_IMPORT: &str = "admin.provider.oauth.batch_import";
pub(crate) const TASK_KEY_SYSTEM_S3_BACKUP: &str = "system.s3.backup";
pub(crate) const TASK_KEY_SYSTEM_S3_BACKUP_WORKER: &str = "system.s3.backup.worker";
pub(crate) const TASK_KEY_USAGE_QUEUE_WORKER: &str = "usage.queue.worker";
pub(crate) const TASK_KEY_USAGE_COUNTER_FLUSH: &str = "usage.counter.flush.worker";
pub(crate) const TASK_KEY_VIDEO_TASK_POLLER: &str = "video.task.poller";
pub(crate) const TASK_KEY_MODEL_FETCH_WORKER: &str = "model.fetch.worker";
pub(crate) const TASK_KEY_PROVIDER_QUOTA_RESET: &str = "provider.quota.reset.worker";
pub(crate) const TASK_KEY_ACCOUNT_SELF_CHECK: &str = "account.self_check.worker";
pub(crate) const TASK_KEY_POOL_SCORE_REBUILD: &str = "pool.score.rebuild.worker";
pub(crate) const TASK_KEY_POOL_QUOTA_PROBE: &str = "pool.quota.probe.worker";
pub(crate) const TASK_KEY_POOL_MONITOR: &str = "pool.monitor.worker";
pub(crate) const TASK_KEY_AUDIT_CLEANUP: &str = "maintenance.audit.cleanup";
pub(crate) const TASK_KEY_DB_MAINTENANCE: &str = "maintenance.database";
pub(crate) const TASK_KEY_PENDING_CLEANUP: &str = "maintenance.pending.cleanup";
pub(crate) const TASK_KEY_REQUEST_CANDIDATE_CLEANUP: &str = "maintenance.request.candidate.cleanup";
pub(crate) const TASK_KEY_GEMINI_FILES_CLEANUP: &str = "maintenance.gemini.files.cleanup";
pub(crate) const TASK_KEY_FIXED_PROVIDER_RECONCILIATION: &str =
    "maintenance.provider.fixed_template.reconcile";
pub(crate) const TASK_KEY_OAUTH_TOKEN_REFRESH: &str = "maintenance.oauth.token.refresh";
pub(crate) const TASK_KEY_PROXY_NODE_STALE_CLEANUP: &str = "maintenance.proxy.node.stale.cleanup";
pub(crate) const TASK_KEY_PROXY_NODE_METRICS_CLEANUP: &str =
    "maintenance.proxy.node.metrics.cleanup";
pub(crate) const TASK_KEY_PROXY_UPGRADE_ROLLOUT: &str = "maintenance.proxy.upgrade.rollout";
pub(crate) const TASK_KEY_PROVIDER_CHECKIN: &str = "maintenance.provider.checkin";
pub(crate) const TASK_KEY_PROVIDER_QUOTA_ALERT: &str = "maintenance.provider.quota_alert";
pub(crate) const TASK_KEY_PROVIDER_REMOTE_QUOTA_SYNC: &str =
    "maintenance.provider.remote_quota.sync";
pub(crate) const TASK_KEY_USAGE_CLEANUP: &str = "maintenance.usage.cleanup";
pub(crate) const TASK_KEY_WALLET_DAILY_USAGE_AGG: &str = "maintenance.wallet.daily.usage.agg";
pub(crate) const TASK_KEY_STATS_DAILY_AGG: &str = "maintenance.stats.daily.agg";
pub(crate) const TASK_KEY_STATS_HOURLY_AGG: &str = "maintenance.stats.hourly.agg";
pub(crate) const TASK_KEY_USAGE_SYNC_REPORT: &str = "usage.sync.report";
pub(crate) const TASK_KEY_PROVIDER_OAUTH_ACCOUNT_REFRESH: &str = "provider.oauth.account.refresh";
pub(crate) const TASK_KEY_PROVIDER_BALANCE_REFRESH: &str = "provider.ops.balance.refresh";
const PROVIDER_DELETE_LOCK_TTL_SECS: u64 = 60 * 60 * 6;

const RETRY_ONCE: RetryPolicy = RetryPolicy { max_attempts: 1 };
const RETRY_THREE: RetryPolicy = RetryPolicy { max_attempts: 3 };
const BACKGROUND_TASK_RUN_ID_MAX_BYTES: usize = 64;
const WORKER_BOOT_RUN_ID_HASH_HEX_BYTES: usize = 20;

fn build_worker_boot_run_id(task_key: &str) -> String {
    let full_run_id = format!("boot:{task_key}");
    if full_run_id.len() <= BACKGROUND_TASK_RUN_ID_MAX_BYTES {
        return full_run_id;
    }

    let digest_hex = format!("{:x}", Sha256::digest(full_run_id.as_bytes()));
    let suffix = &digest_hex[..WORKER_BOOT_RUN_ID_HASH_HEX_BYTES];
    let mut prefix_bytes = BACKGROUND_TASK_RUN_ID_MAX_BYTES - 1 - WORKER_BOOT_RUN_ID_HASH_HEX_BYTES;
    while !full_run_id.is_char_boundary(prefix_bytes) {
        prefix_bytes -= 1;
    }

    format!("{}~{suffix}", &full_run_id[..prefix_bytes])
}

fn build_worker_boot_run(
    task_key: &str,
    kind: BackgroundTaskKind,
    trigger: &str,
    now: u64,
) -> UpsertBackgroundTaskRun {
    UpsertBackgroundTaskRun {
        id: build_worker_boot_run_id(task_key),
        task_key: task_key.to_string(),
        kind,
        trigger: trigger.to_string(),
        status: BackgroundTaskStatus::Running,
        attempt: 1,
        max_attempts: 1,
        // This row represents the logical worker registration shared by every gateway.
        // A supervisor starting does not mean that instance owns the singleton lease.
        owner_instance: None,
        progress_percent: 0,
        progress_message: Some("worker registered".to_string()),
        payload_json: None,
        result_json: None,
        error_message: None,
        cancel_requested: false,
        created_by: Some("system".to_string()),
        created_at_unix_secs: now,
        started_at_unix_secs: Some(now),
        finished_at_unix_secs: None,
        updated_at_unix_secs: now,
    }
}

fn worker_boot_event_payload(gateway_instance_id: &str) -> Value {
    json!({ "gateway_instance_id": gateway_instance_id })
}

pub(crate) fn spawn_singleton_worker<F, Fut>(
    app: AppState,
    task_key: &'static str,
    worker: F,
) -> JoinHandle<()>
where
    F: Fn(AppState) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    spawn_singleton_worker_with_context(app, task_key, move |app, _context| worker(app))
}

pub(crate) fn spawn_singleton_worker_with_context<F, Fut>(
    app: AppState,
    task_key: &'static str,
    worker: F,
) -> JoinHandle<()>
where
    F: Fn(AppState, aether_gateway_workers::SingletonWorkerContext) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let runtime_state = app.runtime_state.clone();
    let metrics = app.task_supervisor_metrics.clone();
    let owner = app.tunnel.local_instance_id().to_string();
    aether_gateway_workers::spawn_singleton_worker_with_context(
        runtime_state,
        metrics,
        owner,
        task_key,
        aether_gateway_workers::SingletonWorkerConfig::default(),
        move |context| worker(app.clone(), context),
    )
}

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
        TASK_KEY_SYSTEM_S3_BACKUP,
        TaskKind::Scheduled,
        "manual",
        false,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_SYSTEM_S3_BACKUP_WORKER,
        TaskKind::Daemon,
        "daemon",
        true,
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
        TASK_KEY_USAGE_COUNTER_FLUSH,
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
        TASK_KEY_ACCOUNT_SELF_CHECK,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_POOL_SCORE_REBUILD,
        TaskKind::Scheduled,
        "interval",
        true,
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
        TASK_KEY_FIXED_PROVIDER_RECONCILIATION,
        TaskKind::FireAndForget,
        "startup",
        true,
        false,
        RETRY_THREE,
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
        TASK_KEY_PROVIDER_QUOTA_ALERT,
        TaskKind::Scheduled,
        "interval",
        true,
        true,
        RETRY_ONCE,
    ),
    TaskDefinition::new(
        TASK_KEY_PROVIDER_REMOTE_QUOTA_SYNC,
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
        let gateway_instance_id = app.tunnel.local_instance_id().to_string();
        let run = build_worker_boot_run(task_key, kind, trigger, now);
        let run_id = run.id.clone();
        if upsert_run_with_logging(&app, run).await.is_none() {
            return;
        }
        append_event_with_logging(
            &app,
            &run_id,
            "worker_boot",
            "background worker supervisor started",
            Some(worker_boot_event_payload(&gateway_instance_id)),
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
    let reserved =
        state
            .as_ref()
            .reserve_provider_delete_task(crate::LocalProviderDeleteTaskState {
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
    if reserved.task_id != task_id {
        return Ok(Some(reserved.task_id));
    }

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
        let lock_ttl = std::time::Duration::from_secs(PROVIDER_DELETE_LOCK_TTL_SECS);
        let lock = app
            .runtime_state
            .lock_try_acquire(&lock_key, app.tunnel.local_instance_id(), lock_ttl)
            .await
            .ok()
            .flatten();
        if lock.is_none() {
            app.put_provider_delete_task(crate::LocalProviderDeleteTaskState {
                task_id: run_id.clone(),
                provider_id: provider_id.clone(),
                status: "failed".to_string(),
                stage: "skipped".to_string(),
                total_keys: 0,
                deleted_keys: 0,
                total_endpoints: 0,
                deleted_endpoints: 0,
                message: "provider delete skipped: another node is running this task".to_string(),
            });
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

#[cfg(test)]
mod worker_boot_run_id_tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use super::{
        build_worker_boot_run, build_worker_boot_run_id, spawn_record_worker_boot,
        worker_boot_event_payload, BACKGROUND_TASK_RUN_ID_MAX_BYTES,
    };
    use aether_data::repository::background_tasks::InMemoryBackgroundTaskRepository;
    use aether_data_contracts::repository::background_tasks::{
        BackgroundTaskKind, BackgroundTaskListQuery, BackgroundTaskReadRepository,
        BackgroundTaskStatus,
    };
    use serde_json::json;

    use crate::{data::GatewayDataState, AppState};

    #[test]
    fn worker_boot_run_id_is_keyed_only_by_task() {
        assert_eq!(
            build_worker_boot_run_id("usage.queue.worker"),
            "boot:usage.queue.worker"
        );
    }

    #[test]
    fn worker_boot_run_id_preserves_exact_database_limit() {
        let task_key = "t".repeat(BACKGROUND_TASK_RUN_ID_MAX_BYTES - "boot:".len());
        let run_id = build_worker_boot_run_id(&task_key);

        assert_eq!(run_id.len(), BACKGROUND_TASK_RUN_ID_MAX_BYTES);
        assert_eq!(run_id, format!("boot:{task_key}"));
    }

    #[test]
    fn worker_boot_run_id_compacts_oversized_values_deterministically() {
        let task_key = "worker-task-".repeat(8);
        let first = build_worker_boot_run_id(&task_key);
        let second = build_worker_boot_run_id(&task_key);

        assert!(first.len() <= BACKGROUND_TASK_RUN_ID_MAX_BYTES);
        assert_eq!(first, second);
        assert!(first.starts_with("boot:worker-task-"));
        assert!(first.contains('~'));
    }

    #[test]
    fn worker_boot_run_id_hash_distinguishes_shared_long_prefixes() {
        let shared_prefix = "worker-task-with-a-very-long-shared-prefix-".repeat(2);
        let first = build_worker_boot_run_id(&format!("{shared_prefix}a"));
        let second = build_worker_boot_run_id(&format!("{shared_prefix}b"));

        assert_ne!(first, second);
    }

    #[test]
    fn worker_boot_run_id_handles_unicode_at_truncation_boundary() {
        let run_id = build_worker_boot_run_id(&"后台任务".repeat(16));

        assert!(run_id.len() <= BACKGROUND_TASK_RUN_ID_MAX_BYTES);
        assert!(run_id.is_char_boundary(run_id.len()));
        assert!(run_id.starts_with("boot:后台任务"));
    }

    #[test]
    fn worker_boot_run_is_a_gateway_neutral_logical_registration() {
        let run = build_worker_boot_run(
            "usage.queue.worker",
            BackgroundTaskKind::Daemon,
            "daemon",
            123,
        );

        assert_eq!(run.id, "boot:usage.queue.worker");
        assert_eq!(run.status, BackgroundTaskStatus::Running);
        assert_eq!(run.owner_instance, None);
        assert_eq!(run.progress_message.as_deref(), Some("worker registered"));
        assert_eq!(run.created_at_unix_secs, 123);
        assert_eq!(run.started_at_unix_secs, Some(123));
        assert_eq!(run.updated_at_unix_secs, 123);
    }

    #[test]
    fn worker_boot_event_keeps_the_observing_gateway_instance() {
        assert_eq!(
            worker_boot_event_payload("gateway-a"),
            json!({ "gateway_instance_id": "gateway-a" })
        );
    }

    #[tokio::test]
    async fn worker_boot_registration_is_shared_but_events_keep_each_gateway() {
        let repository = Arc::new(InMemoryBackgroundTaskRepository::default());
        let state_for = |gateway_instance_id: &str| {
            AppState::new()
                .expect("gateway state should build")
                .with_data_state_for_tests(
                    GatewayDataState::disabled()
                        .with_background_task_repository_for_tests(repository.clone()),
                )
                .with_tunnel_identity_for_tests(gateway_instance_id, None)
        };

        spawn_record_worker_boot(
            state_for("gateway-a"),
            "usage.queue.worker",
            BackgroundTaskKind::Daemon,
            "daemon",
        )
        .await
        .expect("gateway-a worker boot recorder should finish");
        spawn_record_worker_boot(
            state_for("gateway-b"),
            "usage.queue.worker",
            BackgroundTaskKind::Daemon,
            "daemon",
        )
        .await
        .expect("gateway-b worker boot recorder should finish");

        let page = repository
            .list_runs(&BackgroundTaskListQuery {
                task_key_substring: Some("usage.queue.worker".to_string()),
                kind: None,
                status: None,
                trigger: None,
                offset: 0,
                limit: 10,
            })
            .await
            .expect("worker boot runs should load");
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "boot:usage.queue.worker");
        assert_eq!(page.items[0].owner_instance, None);

        let events = repository
            .list_events("boot:usage.queue.worker", 0, 10)
            .await
            .expect("worker boot events should load");
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.event_type == "worker_boot"));
        let gateway_instances = events
            .iter()
            .filter_map(|event| {
                event
                    .payload_json
                    .as_ref()
                    .and_then(|payload| payload.get("gateway_instance_id"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            gateway_instances,
            BTreeSet::from(["gateway-a".to_string(), "gateway-b".to_string()])
        );
    }
}
