use std::fmt;
use std::future::Future;
use std::time::Duration;

use aether_admin::system::admin_system_config_default_value;
use aether_data_contracts::repository::background_tasks::{
    BackgroundTaskKind, BackgroundTaskListQuery, BackgroundTaskStatus, StoredBackgroundTaskRun,
    UpsertBackgroundTaskRun,
};
use aether_runtime_state::RuntimeLockLease;
use axum::http::StatusCode;
use chrono::Utc;
use futures_util::FutureExt;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::task::{JoinError, JoinHandle};
use tracing::warn;

use super::config::S3BackupConfig;
use super::executor::{run_backup_with_store, BackupRunResult};
use super::scopes::BackupScope;
use super::store::ObjectStoreS3BackupStore;
use crate::admin_api::{AdminAppState, SystemExportMode};
use crate::handlers::shared::decrypt_or_migrate_system_config_secret;
use crate::task_runtime::{
    append_event_with_logging, build_task_run_id, now_unix_secs, spawn_fire_and_forget,
    task_definition, update_run_status, upsert_run_with_logging, TASK_KEY_SYSTEM_S3_BACKUP,
};
use crate::{AppState, GatewayError};

const S3_BACKUP_CONFIG_KEYS: &[&str] = &[
    "backup_s3_enabled",
    "backup_s3_scope",
    "backup_s3_endpoint",
    "backup_s3_region",
    "backup_s3_user_agent",
    "backup_s3_bucket",
    "backup_s3_prefix",
    "backup_s3_access_key_id",
    "backup_s3_secret_access_key",
    "backup_s3_path_style",
    "backup_s3_compression",
    "backup_s3_schedule_unit",
    "backup_s3_schedule_interval",
    "backup_s3_schedule_minute",
    "backup_s3_schedule_hour",
    "backup_s3_schedule_weekday",
    "backup_s3_schedule_month_day",
    "backup_s3_retention_count",
];

const S3_BACKUP_QUEUED_MESSAGE: &str = "S3 备份任务已提交";
const S3_BACKUP_INTERNAL_ERROR_DETAIL: &str = "S3 备份服务暂时不可用";
const S3_BACKUP_TASK_FAILURE_CODE: &str = "s3_backup_failed";
const S3_BACKUP_SLOT_RECORD_FAILURE_CODE: &str = "s3_backup_slot_record_failed";
const S3_BACKUP_TASK_LOCK_KEY: &str = "task_runtime:lock:system.s3.backup";
const S3_BACKUP_TASK_LOCK_TTL: Duration = Duration::from_secs(60 * 60 * 6);
const S3_BACKUP_TASK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60 * 5);
const S3_BACKUP_ACTIVE_TASK_STALE_AFTER_SECS: u64 = 60 * 60 * 6;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct S3BackupTaskStart {
    pub(crate) id: String,
    pub(crate) task_key: &'static str,
    pub(crate) status: &'static str,
    pub(crate) progress_message: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct S3BackupTaskError {
    status: StatusCode,
    detail: String,
}

enum BackupLockRenewalFailure<E> {
    Lost,
    Backend(E),
}

enum BackupLockRaceOutcome<T> {
    BackupCompleted(T),
    LeaseLost(Result<(), JoinError>),
}

impl S3BackupTaskError {
    fn bad_request(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            detail: detail.into(),
        }
    }

    fn internal(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail: detail.into(),
        }
    }

    fn service_unavailable(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail: detail.into(),
        }
    }

    fn conflict(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            detail: detail.into(),
        }
    }

    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for S3BackupTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for S3BackupTaskError {}

impl From<GatewayError> for S3BackupTaskError {
    fn from(_error: GatewayError) -> Self {
        warn!(
            error_category = "dependency_failed",
            "S3 backup dependency failed"
        );
        Self::internal(S3_BACKUP_INTERNAL_ERROR_DETAIL)
    }
}

pub(crate) async fn start_s3_backup_task(
    app: AppState,
    trigger: &str,
    created_by: Option<&str>,
) -> Result<S3BackupTaskStart, S3BackupTaskError> {
    start_s3_backup_task_with_slot(app, trigger, created_by, None).await
}

pub(crate) async fn start_s3_backup_task_for_schedule(
    app: AppState,
    scheduled_slot: String,
) -> Result<S3BackupTaskStart, S3BackupTaskError> {
    start_s3_backup_task_with_slot(app, "scheduled", None, Some(scheduled_slot)).await
}

async fn start_s3_backup_task_with_slot(
    app: AppState,
    trigger: &str,
    created_by: Option<&str>,
    scheduled_slot: Option<String>,
) -> Result<S3BackupTaskStart, S3BackupTaskError> {
    let config = load_s3_backup_config_for_run(&app).await?;
    ensure_background_task_storage(&app)?;
    let lock = acquire_s3_backup_task_lock(&app).await?;
    let active_run_exists = match has_active_s3_backup_task(&app).await {
        Ok(value) => value,
        Err(error) => {
            release_s3_backup_task_lock(&app, lock).await;
            return Err(error);
        }
    };
    if active_run_exists {
        release_s3_backup_task_lock(&app, lock).await;
        return Err(S3BackupTaskError::conflict(
            "已有 S3 备份任务正在执行，请等待当前任务完成后再试",
        ));
    }

    let run_id = build_task_run_id();
    let created_at = now_unix_secs();
    let max_attempts = task_definition(TASK_KEY_SYSTEM_S3_BACKUP)
        .map(|item| item.retry_policy.max_attempts)
        .unwrap_or(1);

    let run = UpsertBackgroundTaskRun {
        id: run_id.clone(),
        task_key: TASK_KEY_SYSTEM_S3_BACKUP.to_string(),
        kind: BackgroundTaskKind::Scheduled,
        trigger: trigger.to_string(),
        status: BackgroundTaskStatus::Queued,
        attempt: 1,
        max_attempts,
        owner_instance: Some(app.tunnel.local_instance_id().to_string()),
        progress_percent: 0,
        progress_message: Some(S3_BACKUP_QUEUED_MESSAGE.to_string()),
        payload_json: Some(s3_backup_task_payload_json(
            &config,
            trigger,
            scheduled_slot.as_deref(),
        )),
        result_json: None,
        error_message: None,
        cancel_requested: false,
        created_by: Some(created_by.unwrap_or("admin").to_string()),
        created_at_unix_secs: created_at,
        started_at_unix_secs: None,
        finished_at_unix_secs: None,
        updated_at_unix_secs: created_at,
    };
    if upsert_run_with_logging(&app, run).await.is_none() {
        release_s3_backup_task_lock(&app, lock).await;
        return Err(S3BackupTaskError::service_unavailable(
            "无法创建 S3 备份后台任务记录，请检查后台任务存储是否可用",
        ));
    }
    append_event_with_logging(
        &app,
        &run_id,
        "queued",
        "S3 backup task queued",
        Some(json!({ "trigger": trigger })),
    )
    .await;

    spawn_s3_backup_worker(app, run_id.clone(), config, lock, scheduled_slot);

    Ok(S3BackupTaskStart {
        id: run_id,
        task_key: TASK_KEY_SYSTEM_S3_BACKUP,
        status: BackgroundTaskStatus::Queued.as_database(),
        progress_message: S3_BACKUP_QUEUED_MESSAGE,
    })
}

fn s3_backup_task_payload_json(
    config: &S3BackupConfig,
    trigger: &str,
    scheduled_slot: Option<&str>,
) -> Value {
    let mut payload = json!({
        "scope": config.scope.as_config_value(),
        "compression": config.compression.clone(),
        "trigger": trigger,
    });
    if let Some(scheduled_slot) = scheduled_slot {
        payload["scheduled_slot"] = Value::String(scheduled_slot.to_string());
    }
    payload
}

fn spawn_s3_backup_worker(
    app: AppState,
    run_id: String,
    config: S3BackupConfig,
    lock: RuntimeLockLease,
    scheduled_slot: Option<String>,
) {
    spawn_fire_and_forget("task-runtime-system-s3-backup", async move {
        let app_for_worker = app.clone();
        let run_id_for_worker = run_id.clone();
        let result = std::panic::AssertUnwindSafe(run_s3_backup_worker_inner(
            app_for_worker,
            run_id_for_worker,
            config,
            lock.clone(),
            scheduled_slot,
        ))
        .catch_unwind()
        .await;
        if result.is_err() {
            warn!(run_id = %run_id, "S3 backup task panicked");
            let _ = update_run_status(
                &app,
                &run_id,
                BackgroundTaskStatus::Failed,
                Some(100),
                Some("S3 备份任务异常退出".to_string()),
                None,
                Some("background_task_panicked".to_string()),
                None,
                Some(now_unix_secs()),
            )
            .await;
            append_event_with_logging(&app, &run_id, "failed", "S3 backup task panicked", None)
                .await;
        }

        release_s3_backup_task_lock(&app, lock).await;
    });
}

async fn run_s3_backup_worker_inner(
    app: AppState,
    run_id: String,
    config: S3BackupConfig,
    lock: RuntimeLockLease,
    scheduled_slot: Option<String>,
) {
    let started_at = now_unix_secs();
    let _ = update_run_status(
        &app,
        &run_id,
        BackgroundTaskStatus::Running,
        Some(5),
        Some("S3 备份任务开始执行".to_string()),
        None,
        None,
        Some(started_at),
        None,
    )
    .await;
    append_event_with_logging(&app, &run_id, "running", "S3 backup task started", None).await;

    let heartbeat = spawn_s3_backup_task_heartbeat(app.clone(), run_id.clone(), lock.clone());
    let result = match race_backup_with_lock_heartbeat(run_s3_backup_once(&app, &config), heartbeat)
        .await
    {
        BackupLockRaceOutcome::BackupCompleted(result) => {
            match require_successful_backup_lock_renewal(
                app.runtime_state
                    .lock_renew(&lock, S3_BACKUP_TASK_LOCK_TTL)
                    .await,
            ) {
                Ok(()) => result,
                Err(BackupLockRenewalFailure::Lost) => {
                    warn!(
                        run_id = %run_id,
                        lock_key = %lock.key,
                        "S3 backup task lost its distributed lock before publishing completion"
                    );
                    Err(S3BackupTaskError::service_unavailable(
                        "S3 备份任务锁已失效，任务完成状态未发布",
                    ))
                }
                Err(BackupLockRenewalFailure::Backend(error)) => {
                    warn!(
                        run_id = %run_id,
                        lock_key = %lock.key,
                        error = %error,
                        "S3 backup task could not verify its distributed lock before publishing completion"
                    );
                    Err(S3BackupTaskError::service_unavailable(
                        "无法确认 S3 备份任务锁所有权，任务完成状态未发布",
                    ))
                }
            }
        }
        BackupLockRaceOutcome::LeaseLost(heartbeat_result) => {
            match heartbeat_result {
                Ok(()) => warn!(
                    run_id = %run_id,
                    "S3 backup task stopped after losing its distributed lock"
                ),
                Err(error) => warn!(
                    run_id = %run_id,
                    error = %error,
                    "S3 backup lock heartbeat task failed"
                ),
            }
            Err(S3BackupTaskError::service_unavailable(
                "S3 备份任务锁已失效，任务已停止",
            ))
        }
    };

    match result {
        Ok(result) => {
            if let Some(slot) = scheduled_backup_slot_to_record(scheduled_slot.as_deref(), true) {
                if record_scheduled_backup_slot(&app, &slot).await.is_err() {
                    warn!(
                        error_category = "slot_record_failed",
                        run_id = %run_id,
                        "S3 backup slot record failed"
                    );
                    let _ = update_run_status(
                        &app,
                        &run_id,
                        BackgroundTaskStatus::Failed,
                        Some(100),
                        Some("S3 备份任务完成，但记录调度时间失败".to_string()),
                        None,
                        Some(S3_BACKUP_SLOT_RECORD_FAILURE_CODE.to_string()),
                        None,
                        Some(now_unix_secs()),
                    )
                    .await;
                    append_event_with_logging(
                        &app,
                        &run_id,
                        "failed",
                        "S3 backup slot record failed",
                        Some(json!({ "error_code": S3_BACKUP_SLOT_RECORD_FAILURE_CODE })),
                    )
                    .await;
                    return;
                }
            }
            let result_json = backup_run_result_json(&result);
            let _ = update_run_status(
                &app,
                &run_id,
                BackgroundTaskStatus::Succeeded,
                Some(100),
                Some("S3 备份任务完成".to_string()),
                Some(result_json.clone()),
                None,
                None,
                Some(now_unix_secs()),
            )
            .await;
            append_event_with_logging(
                &app,
                &run_id,
                "succeeded",
                "S3 backup task completed",
                Some(result_json),
            )
            .await;
        }
        Err(_) => {
            warn!(
                error_category = "backup_execution_failed",
                run_id = %run_id,
                "S3 backup task failed"
            );
            let _ = update_run_status(
                &app,
                &run_id,
                BackgroundTaskStatus::Failed,
                Some(100),
                Some("S3 备份任务失败".to_string()),
                None,
                Some(S3_BACKUP_TASK_FAILURE_CODE.to_string()),
                None,
                Some(now_unix_secs()),
            )
            .await;
            append_event_with_logging(
                &app,
                &run_id,
                "failed",
                "S3 backup task failed",
                Some(json!({ "error_code": S3_BACKUP_TASK_FAILURE_CODE })),
            )
            .await;
        }
    }
}

async fn race_backup_with_lock_heartbeat<F, T>(
    backup: F,
    mut heartbeat: JoinHandle<()>,
) -> BackupLockRaceOutcome<T>
where
    F: Future<Output = T>,
{
    tokio::pin!(backup);
    tokio::select! {
        biased;
        heartbeat_result = &mut heartbeat => {
            BackupLockRaceOutcome::LeaseLost(heartbeat_result)
        }
        result = &mut backup => {
            heartbeat.abort();
            let _ = heartbeat.await;
            BackupLockRaceOutcome::BackupCompleted(result)
        }
    }
}

fn require_successful_backup_lock_renewal<E>(
    result: Result<bool, E>,
) -> Result<(), BackupLockRenewalFailure<E>> {
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(BackupLockRenewalFailure::Lost),
        Err(error) => Err(BackupLockRenewalFailure::Backend(error)),
    }
}

fn spawn_s3_backup_task_heartbeat(
    app: AppState,
    run_id: String,
    lock: RuntimeLockLease,
) -> tokio::task::JoinHandle<()> {
    spawn_fire_and_forget("task-runtime-system-s3-backup-heartbeat", async move {
        let mut interval = tokio::time::interval(S3_BACKUP_TASK_HEARTBEAT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            match require_successful_backup_lock_renewal(
                app.runtime_state
                    .lock_renew(&lock, S3_BACKUP_TASK_LOCK_TTL)
                    .await,
            ) {
                Ok(()) => {}
                Err(BackupLockRenewalFailure::Lost) => {
                    warn!(
                        run_id = %run_id,
                        lock_key = %lock.key,
                        "S3 backup task distributed lock is no longer owned"
                    );
                    return;
                }
                Err(BackupLockRenewalFailure::Backend(error)) => {
                    warn!(
                        run_id = %run_id,
                        lock_key = %lock.key,
                        error = %error,
                        "S3 backup task distributed lock renewal failed"
                    );
                    return;
                }
            }
            let _ = update_run_status(
                &app,
                &run_id,
                BackgroundTaskStatus::Running,
                Some(50),
                Some("S3 备份任务执行中".to_string()),
                None,
                None,
                None,
                None,
            )
            .await;
        }
    })
}

fn scheduled_backup_slot_to_record(
    scheduled_slot: Option<&str>,
    task_succeeded: bool,
) -> Option<String> {
    if !task_succeeded {
        return None;
    }
    scheduled_slot
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn record_scheduled_backup_slot(
    app: &AppState,
    scheduled_slot: &str,
) -> Result<(), GatewayError> {
    app.upsert_system_config_json_value(
        super::S3_BACKUP_LAST_SLOT_KEY,
        &Value::String(scheduled_slot.to_string()),
        None,
    )
    .await?;
    Ok(())
}

fn ensure_background_task_storage(app: &AppState) -> Result<(), S3BackupTaskError> {
    if app.has_background_task_data_reader() && app.has_background_task_data_writer() {
        return Ok(());
    }

    Err(S3BackupTaskError::service_unavailable(
        "当前节点未启用后台任务存储，无法提交 S3 备份任务",
    ))
}

async fn acquire_s3_backup_task_lock(
    app: &AppState,
) -> Result<RuntimeLockLease, S3BackupTaskError> {
    match app
        .runtime_state
        .lock_try_acquire(
            S3_BACKUP_TASK_LOCK_KEY,
            app.tunnel.local_instance_id(),
            S3_BACKUP_TASK_LOCK_TTL,
        )
        .await
    {
        Ok(Some(lock)) => Ok(lock),
        Ok(None) => Err(S3BackupTaskError::conflict(
            "已有 S3 备份任务正在执行，请等待当前任务完成后再试",
        )),
        Err(_) => {
            warn!(
                error_category = "lock_acquisition_failed",
                "S3 backup task lock acquisition failed"
            );
            Err(S3BackupTaskError::service_unavailable(
                "无法获取 S3 备份任务锁，请稍后重试",
            ))
        }
    }
}

async fn release_s3_backup_task_lock(app: &AppState, lock: RuntimeLockLease) {
    let _ = app.runtime_state.lock_release(&lock).await;
}

async fn has_active_s3_backup_task(app: &AppState) -> Result<bool, S3BackupTaskError> {
    let now = now_unix_secs();
    for status in [BackgroundTaskStatus::Queued, BackgroundTaskStatus::Running] {
        let page = app
            .list_background_task_runs(&BackgroundTaskListQuery {
                task_key_substring: Some(TASK_KEY_SYSTEM_S3_BACKUP.to_string()),
                kind: Some(BackgroundTaskKind::Scheduled),
                status: Some(status),
                trigger: None,
                offset: 0,
                limit: 100,
            })
            .await?;
        if page
            .items
            .iter()
            .any(|run| is_blocking_active_s3_backup_run(run, status, now))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_blocking_active_s3_backup_run(
    run: &StoredBackgroundTaskRun,
    status: BackgroundTaskStatus,
    now_unix_secs: u64,
) -> bool {
    run.task_key == TASK_KEY_SYSTEM_S3_BACKUP
        && run.kind == BackgroundTaskKind::Scheduled
        && run.status == status
        && !run.cancel_requested
        && now_unix_secs.saturating_sub(run.updated_at_unix_secs)
            < S3_BACKUP_ACTIVE_TASK_STALE_AFTER_SECS
}

async fn run_s3_backup_once(
    app: &AppState,
    config: &S3BackupConfig,
) -> Result<BackupRunResult, S3BackupTaskError> {
    let Some(encryption_secret) = effective_backup_encryption_secret(app) else {
        return Err(S3BackupTaskError::service_unavailable(
            "S3 备份需要 AETHER_BACKUP_ENCRYPTION_KEY 或可用的数据加密密钥",
        ));
    };
    let payload = build_s3_backup_payload_exclusively(app, config.scope).await?;
    let store = ObjectStoreS3BackupStore::from_config(config).map_err(|_| {
        warn!(
            error_category = "object_store_initialization_failed",
            "S3 backup object store initialization failed"
        );
        S3BackupTaskError::internal(S3_BACKUP_INTERNAL_ERROR_DETAIL)
    })?;
    run_backup_with_store(config, &store, payload, Utc::now(), &encryption_secret)
        .await
        .map_err(|_| {
            warn!(
                error_category = "backup_execution_failed",
                "S3 backup execution failed"
            );
            S3BackupTaskError::internal(S3_BACKUP_INTERNAL_ERROR_DETAIL)
        })
}

async fn build_s3_backup_payload_exclusively(
    app: &AppState,
    scope: BackupScope,
) -> Result<Value, S3BackupTaskError> {
    let admin_state = AdminAppState::new(app);
    crate::admin_api::execute_admin_system_import_exclusively(app, async {
        match scope {
            BackupScope::Config => {
                admin_state
                    .build_admin_system_config_export_payload(SystemExportMode::RecoveryBackup)
                    .await
            }
            BackupScope::Users => {
                admin_state
                    .build_admin_system_users_export_payload(SystemExportMode::RecoveryBackup)
                    .await
            }
            BackupScope::Data => {
                admin_state
                    .build_admin_system_data_export_payload(SystemExportMode::RecoveryBackup)
                    .await
            }
        }
    })
    .await
    .map_err(|error| {
        warn!(
            error_category = "system_import_coordination_failed",
            lock_error = ?error,
            "S3 backup snapshot could not acquire or retain the system import lock"
        );
        S3BackupTaskError::service_unavailable(S3_BACKUP_INTERNAL_ERROR_DETAIL)
    })?
    .map_err(S3BackupTaskError::from)
}

fn effective_backup_encryption_secret(app: &AppState) -> Option<String> {
    std::env::var("AETHER_BACKUP_ENCRYPTION_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            app.encryption_key()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

async fn load_s3_backup_config_for_run(
    app: &AppState,
) -> Result<S3BackupConfig, S3BackupTaskError> {
    let mut values = load_s3_backup_config_values(app).await?;
    values.insert("backup_s3_enabled".to_string(), Value::Bool(true));
    S3BackupConfig::from_json_map(&values)
        .map_err(|error| S3BackupTaskError::bad_request(format!("S3 备份配置无效：{error}")))
}

pub(crate) async fn load_s3_backup_config_values(
    app: &AppState,
) -> Result<Map<String, Value>, S3BackupTaskError> {
    let mut values = Map::new();
    for key in S3_BACKUP_CONFIG_KEYS {
        let value = app
            .read_system_config_json_value(key)
            .await
            .map_err(S3BackupTaskError::from)?
            .or_else(|| admin_system_config_default_value(key));
        if let Some(value) = value {
            let value = if *key == "backup_s3_secret_access_key" {
                decrypt_s3_secret_access_key(app, value).await?
            } else {
                value
            };
            values.insert((*key).to_string(), value);
        }
    }
    Ok(values)
}

async fn decrypt_s3_secret_access_key(
    app: &AppState,
    value: Value,
) -> Result<Value, S3BackupTaskError> {
    let Some(stored_value) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(value);
    };
    let plaintext = decrypt_or_migrate_system_config_secret(
        app,
        "backup_s3_secret_access_key",
        stored_value.to_string(),
    )
    .await
    .map_err(|_| {
        S3BackupTaskError::bad_request(
            "S3 备份配置无效：Secret Access Key（访问密钥）无法解密，请重新填写",
        )
    })?;
    Ok(Value::String(plaintext))
}

fn backup_run_result_json(result: &BackupRunResult) -> Value {
    json!({
        "scope": result.scope.as_config_value(),
        "bytes": result.bytes,
        "sha256": result.sha256,
        "export_version": result.export_version,
        "exported_at": result.exported_at,
        "compression": result.compression,
        "encryption": result.encryption,
        "legacy_encrypted_copies_created": result.legacy_encrypted_copies_created,
        "legacy_encrypted_copies_verified": result.legacy_encrypted_copies_verified,
        "legacy_plaintext_objects_deleted": result.legacy_plaintext_objects_deleted,
        "legacy_plaintext_objects_retained": result.legacy_plaintext_objects_retained,
        "retention_cleanup_candidates": result.retention_cleanup_candidates,
        "automatic_deletions": result.legacy_plaintext_objects_deleted,
        "object_cleanup_mode": "legacy_plaintext_deleted_after_verified_encryption",
        "versioned_storage_cleanup_required": result.versioned_storage_cleanup_required,
        "versioned_storage_cleanup_notice": "legacy_plaintext_versions_require_external_cleanup",
    })
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::background_tasks::InMemoryBackgroundTaskRepository;
    use aether_data_contracts::repository::background_tasks::{
        BackgroundTaskKind, BackgroundTaskStatus, StoredBackgroundTaskRun,
    };

    use crate::data::GatewayDataState;
    use crate::handlers::shared::decrypt_system_config_secret;
    use crate::state::AppState;
    use crate::task_runtime::{now_unix_secs, TASK_KEY_SYSTEM_S3_BACKUP};

    #[test]
    fn backup_lock_renewal_requires_ownership_and_preserves_backend_errors() {
        assert!(matches!(
            super::require_successful_backup_lock_renewal::<Infallible>(Ok(true)),
            Ok(())
        ));
        assert!(matches!(
            super::require_successful_backup_lock_renewal::<Infallible>(Ok(false)),
            Err(super::BackupLockRenewalFailure::Lost)
        ));
        assert!(matches!(
            super::require_successful_backup_lock_renewal(Err("redis unavailable")),
            Err(super::BackupLockRenewalFailure::Backend(
                "redis unavailable"
            ))
        ));
    }

    #[tokio::test]
    async fn lost_backup_lock_stops_race_without_publishing_backup_result() {
        let destructive_stage_reached = Arc::new(AtomicBool::new(false));
        let destructive_stage_for_backup = Arc::clone(&destructive_stage_reached);
        let backup = async move {
            std::future::pending::<()>().await;
            destructive_stage_for_backup.store(true, Ordering::Release);
            Ok::<(), super::S3BackupTaskError>(())
        };
        let heartbeat = tokio::spawn(async {});
        let outcome = super::race_backup_with_lock_heartbeat(backup, heartbeat).await;
        assert!(matches!(
            outcome,
            super::BackupLockRaceOutcome::LeaseLost(Ok(()))
        ));
        assert!(!destructive_stage_reached.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn completed_heartbeat_wins_when_backup_completion_is_also_ready() {
        let heartbeat = tokio::spawn(async {});
        tokio::task::yield_now().await;

        let outcome = super::race_backup_with_lock_heartbeat(async { 42_u8 }, heartbeat).await;

        assert!(matches!(
            outcome,
            super::BackupLockRaceOutcome::LeaseLost(Ok(()))
        ));
    }

    #[tokio::test]
    async fn s3_backup_snapshot_refuses_to_overlap_system_import() {
        let app = AppState::new().expect("app state should build");
        let lease = crate::admin_api::try_acquire_admin_system_import_lease(&app)
            .await
            .expect("test should acquire the system import lease");

        let error = super::build_s3_backup_payload_exclusively(
            &app,
            crate::backup::scopes::BackupScope::Config,
        )
        .await
        .expect_err("backup snapshot must not overlap a system import");

        crate::admin_api::release_admin_system_import_lease(&app, &lease).await;
        assert_eq!(error.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.detail(), super::S3_BACKUP_INTERNAL_ERROR_DETAIL);
    }

    fn valid_s3_backup_config_values() -> Vec<(String, serde_json::Value)> {
        vec![
            (
                "backup_s3_endpoint".to_string(),
                serde_json::json!("https://s3.example.com"),
            ),
            (
                "backup_s3_bucket".to_string(),
                serde_json::json!("aether-backups"),
            ),
            ("backup_s3_prefix".to_string(), serde_json::json!("prod/")),
            (
                "backup_s3_access_key_id".to_string(),
                serde_json::json!("access-key-id"),
            ),
            (
                "backup_s3_secret_access_key".to_string(),
                serde_json::json!(encrypt_python_fernet_plaintext(
                    DEVELOPMENT_ENCRYPTION_KEY,
                    "secret"
                )
                .expect("test secret should encrypt")),
            ),
        ]
    }

    #[tokio::test]
    async fn legacy_plaintext_s3_secret_is_migrated_when_config_loads() {
        let plaintext = "legacy-s3-secret-access-key";
        let mut entries = valid_s3_backup_config_values();
        entries
            .iter_mut()
            .find(|(key, _)| key == "backup_s3_secret_access_key")
            .expect("secret config fixture should exist")
            .1 = serde_json::json!(plaintext);
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
                    .with_system_config_values_for_tests(entries),
            );

        let values = super::load_s3_backup_config_values(&app)
            .await
            .expect("legacy S3 config should load");
        assert_eq!(
            values.get("backup_s3_secret_access_key"),
            Some(&serde_json::json!(plaintext))
        );

        let stored = app
            .read_system_config_json_value_strong("backup_s3_secret_access_key")
            .await
            .expect("stored S3 secret should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("stored S3 secret should remain a string");
        assert_ne!(stored, plaintext);
        assert_eq!(
            decrypt_system_config_secret(&app, "backup_s3_secret_access_key", &stored)
                .expect("migrated S3 secret should decrypt"),
            plaintext
        );
    }

    #[tokio::test]
    async fn undecryptable_s3_fernet_secret_fails_closed() {
        let plaintext = "s3-secret-from-unavailable-key";
        let ciphertext = encrypt_python_fernet_plaintext("unavailable-s3-key", plaintext)
            .expect("unknown-key fixture should encrypt");
        let mut entries = valid_s3_backup_config_values();
        entries
            .iter_mut()
            .find(|(key, _)| key == "backup_s3_secret_access_key")
            .expect("secret config fixture should exist")
            .1 = serde_json::json!(ciphertext.clone());
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
                    .with_system_config_values_for_tests(entries),
            );

        let error = super::load_s3_backup_config_values(&app)
            .await
            .expect_err("unknown-key S3 ciphertext must fail closed");
        let error_text = error.to_string();
        assert!(!error_text.contains(plaintext));
        assert!(!error_text.contains(&ciphertext));
        assert_eq!(
            app.read_system_config_json_value_strong("backup_s3_secret_access_key")
                .await
                .expect("stored S3 secret should read"),
            Some(serde_json::json!(ciphertext))
        );
    }

    #[test]
    fn gateway_dependency_errors_are_not_exposed_to_backup_clients() {
        let error = super::S3BackupTaskError::from(crate::GatewayError::Internal(
            "postgresql://admin:database-secret@db.internal/aether".to_string(),
        ));

        assert_eq!(
            error.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(error.detail(), super::S3_BACKUP_INTERNAL_ERROR_DETAIL);
        assert!(!error.detail().contains("database-secret"));
    }

    fn stored_s3_backup_run(status: BackgroundTaskStatus) -> StoredBackgroundTaskRun {
        let now = now_unix_secs();
        StoredBackgroundTaskRun {
            id: "existing-s3-backup-run".to_string(),
            task_key: TASK_KEY_SYSTEM_S3_BACKUP.to_string(),
            kind: BackgroundTaskKind::Scheduled,
            trigger: "manual".to_string(),
            status,
            attempt: 1,
            max_attempts: 1,
            owner_instance: Some("test-instance".to_string()),
            progress_percent: 5,
            progress_message: Some("running".to_string()),
            payload_json: None,
            result_json: None,
            error_message: None,
            cancel_requested: false,
            created_by: Some("admin".to_string()),
            created_at_unix_secs: now,
            started_at_unix_secs: Some(now),
            finished_at_unix_secs: None,
            updated_at_unix_secs: now,
        }
    }

    #[tokio::test]
    async fn start_s3_backup_task_rejects_missing_bucket_for_manual_run() {
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests(vec![(
                    "backup_s3_endpoint".to_string(),
                    serde_json::json!("https://s3.example.com"),
                )]),
            );

        let err = super::start_s3_backup_task(app, "manual", Some("admin-user-123"))
            .await
            .expect_err("missing bucket should reject the backup run");

        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(err.to_string().contains("Bucket"));
    }

    #[tokio::test]
    async fn start_s3_backup_task_requires_background_task_storage() {
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
                    .with_system_config_values_for_tests(valid_s3_backup_config_values()),
            );

        let err = super::start_s3_backup_task(app, "manual", Some("admin-user-123"))
            .await
            .expect_err("manual backup should require observable background task storage");

        assert_eq!(err.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(err.to_string().contains("后台任务存储"));
    }

    #[tokio::test]
    async fn start_s3_backup_task_rejects_when_same_task_is_active() {
        let repository = Arc::new(InMemoryBackgroundTaskRepository::seed_runs([
            stored_s3_backup_run(BackgroundTaskStatus::Running),
        ]));
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
                    .with_system_config_values_for_tests(valid_s3_backup_config_values())
                    .with_background_task_repository_for_tests(repository),
            );

        let err = super::start_s3_backup_task(app, "manual", Some("admin-user-123"))
            .await
            .expect_err("manual backup should reject duplicate active runs");

        assert_eq!(err.status(), axum::http::StatusCode::CONFLICT);
        assert!(err.to_string().contains("已有 S3 备份任务正在执行"));
    }

    #[tokio::test]
    async fn active_s3_backup_detection_blocks_queued_runs() {
        let repository = Arc::new(InMemoryBackgroundTaskRepository::seed_runs([
            stored_s3_backup_run(BackgroundTaskStatus::Queued),
        ]));
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_background_task_repository_for_tests(repository),
            );

        assert!(super::has_active_s3_backup_task(&app)
            .await
            .expect("active task lookup should succeed"));
    }

    #[tokio::test]
    async fn active_s3_backup_detection_ignores_cancelled_and_stale_runs() {
        let now = now_unix_secs();
        let mut stale = stored_s3_backup_run(BackgroundTaskStatus::Running);
        stale.id = "stale-s3-backup-run".to_string();
        stale.updated_at_unix_secs = now
            .saturating_sub(super::S3_BACKUP_ACTIVE_TASK_STALE_AFTER_SECS)
            .saturating_sub(1);
        let mut cancelled = stored_s3_backup_run(BackgroundTaskStatus::Queued);
        cancelled.id = "cancelled-s3-backup-run".to_string();
        cancelled.cancel_requested = true;
        let repository = Arc::new(InMemoryBackgroundTaskRepository::seed_runs([
            stale, cancelled,
        ]));
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_background_task_repository_for_tests(repository),
            );

        assert!(!super::has_active_s3_backup_task(&app)
            .await
            .expect("active task lookup should succeed"));
    }

    #[tokio::test]
    async fn queued_s3_backup_task_payload_does_not_include_secret() {
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
                    .with_system_config_values_for_tests(valid_s3_backup_config_values()),
            );
        let values = super::load_s3_backup_config_values(&app)
            .await
            .expect("config should load");
        let config = super::S3BackupConfig::from_json_map(&values)
            .expect("config should parse for payload test");

        let payload = super::s3_backup_task_payload_json(&config, "manual", None);

        assert!(payload.get("bucket").is_none());
        assert!(payload.get("prefix").is_none());
        assert_eq!(payload["scope"], serde_json::json!("data"));
        assert_eq!(payload["trigger"], serde_json::json!("manual"));
        assert!(!payload.to_string().contains("secret"));
    }

    #[test]
    fn scheduled_backup_slot_records_only_successful_scheduled_runs() {
        assert_eq!(
            super::scheduled_backup_slot_to_record(Some("days:2026-05-24T19:00:00Z"), true),
            Some("days:2026-05-24T19:00:00Z".to_string())
        );
        assert_eq!(
            super::scheduled_backup_slot_to_record(Some("days:2026-05-24T19:00:00Z"), false),
            None
        );
        assert_eq!(super::scheduled_backup_slot_to_record(None, true), None);
        assert_eq!(
            super::scheduled_backup_slot_to_record(Some("  "), true),
            None
        );
    }

    #[tokio::test]
    async fn record_scheduled_backup_slot_updates_system_config() {
        let app = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests(Vec::<(
                    String,
                    serde_json::Value,
                )>::new(
                )),
            );

        super::record_scheduled_backup_slot(&app, "days:2026-05-24T19:00:00Z")
            .await
            .expect("slot record should write system config");

        assert_eq!(
            app.read_system_config_json_value(super::super::S3_BACKUP_LAST_SLOT_KEY)
                .await
                .expect("slot config should be readable"),
            Some(serde_json::json!("days:2026-05-24T19:00:00Z"))
        );
    }
}
