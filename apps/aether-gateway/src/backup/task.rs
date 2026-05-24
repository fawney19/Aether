use std::fmt;

use aether_admin::system::admin_system_config_default_value;
use aether_data_contracts::repository::background_tasks::{
    BackgroundTaskKind, BackgroundTaskStatus, UpsertBackgroundTaskRun,
};
use axum::http::StatusCode;
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tracing::warn;

use super::config::S3BackupConfig;
use super::executor::{run_backup_with_store, BackupRunResult};
use super::scopes::BackupScope;
use super::store::ObjectStoreS3BackupStore;
use crate::admin_api::AdminAppState;
use crate::handlers::shared::decrypt_catalog_secret_with_fallbacks;
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
    fn from(error: GatewayError) -> Self {
        Self::internal(format!("{error:?}"))
    }
}

pub(crate) async fn start_s3_backup_task(
    app: AppState,
    trigger: &str,
    created_by: Option<&str>,
) -> Result<S3BackupTaskStart, S3BackupTaskError> {
    let config = load_s3_backup_config_for_run(&app).await?;
    let run_id = build_task_run_id();
    let created_at = now_unix_secs();
    let max_attempts = task_definition(TASK_KEY_SYSTEM_S3_BACKUP)
        .map(|item| item.retry_policy.max_attempts)
        .unwrap_or(1);

    if app.has_background_task_data_writer() {
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
            payload_json: Some(json!({
                "scope": config.scope.as_config_value(),
                "bucket": config.bucket.clone(),
                "prefix": config.prefix.clone(),
                "compression": config.compression.clone(),
                "trigger": trigger,
            })),
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
            "S3 backup task queued",
            Some(json!({ "trigger": trigger })),
        )
        .await;
    }

    spawn_s3_backup_worker(app, run_id.clone(), config);

    Ok(S3BackupTaskStart {
        id: run_id,
        task_key: TASK_KEY_SYSTEM_S3_BACKUP,
        status: BackgroundTaskStatus::Queued.as_database(),
        progress_message: S3_BACKUP_QUEUED_MESSAGE,
    })
}

fn spawn_s3_backup_worker(app: AppState, run_id: String, config: S3BackupConfig) {
    spawn_fire_and_forget("task-runtime-system-s3-backup", async move {
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

        match run_s3_backup_once(&app, &config).await {
            Ok(result) => {
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
            Err(error) => {
                warn!(error = %error, run_id = %run_id, "S3 backup task failed");
                let _ = update_run_status(
                    &app,
                    &run_id,
                    BackgroundTaskStatus::Failed,
                    Some(100),
                    Some("S3 备份任务失败".to_string()),
                    None,
                    Some(error.to_string()),
                    None,
                    Some(now_unix_secs()),
                )
                .await;
                append_event_with_logging(
                    &app,
                    &run_id,
                    "failed",
                    "S3 backup task failed",
                    Some(json!({ "error": error.to_string() })),
                )
                .await;
            }
        }
    });
}

async fn run_s3_backup_once(
    app: &AppState,
    config: &S3BackupConfig,
) -> Result<BackupRunResult, S3BackupTaskError> {
    let admin_state = AdminAppState::new(app);
    let payload = match config.scope {
        BackupScope::Config => {
            admin_state
                .build_admin_system_config_export_payload()
                .await?
        }
        BackupScope::Users => {
            admin_state
                .build_admin_system_users_export_payload()
                .await?
        }
        BackupScope::Data => admin_state.build_admin_system_data_export_payload().await?,
    };
    let store = ObjectStoreS3BackupStore::from_config(config)
        .map_err(|error| S3BackupTaskError::internal(error.to_string()))?;
    run_backup_with_store(config, &store, payload, Utc::now())
        .await
        .map_err(|error| S3BackupTaskError::internal(error.to_string()))
}

async fn load_s3_backup_config_for_run(
    app: &AppState,
) -> Result<S3BackupConfig, S3BackupTaskError> {
    let mut values = load_s3_backup_config_values(app).await?;
    values.insert("backup_s3_enabled".to_string(), Value::Bool(true));
    S3BackupConfig::from_json_map(&values)
        .map_err(|error| S3BackupTaskError::bad_request(format!("S3 备份配置无效：{error}")))
}

async fn load_s3_backup_config_values(
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
                decrypt_s3_secret_access_key(app, value)?
            } else {
                value
            };
            values.insert((*key).to_string(), value);
        }
    }
    Ok(values)
}

fn decrypt_s3_secret_access_key(app: &AppState, value: Value) -> Result<Value, S3BackupTaskError> {
    let Some(ciphertext) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(value);
    };
    let Some(plaintext) = decrypt_catalog_secret_with_fallbacks(app.encryption_key(), ciphertext)
    else {
        return Err(S3BackupTaskError::bad_request(
            "S3 备份配置无效：Secret Access Key（访问密钥）无法解密，请重新填写",
        ));
    };
    Ok(Value::String(plaintext))
}

fn backup_run_result_json(result: &BackupRunResult) -> Value {
    json!({
        "scope": result.scope.as_config_value(),
        "bucket": result.bucket,
        "object_key": result.object_key,
        "bytes": result.bytes,
        "sha256": result.sha256,
        "export_version": result.export_version,
        "exported_at": result.exported_at,
        "compression": result.compression,
        "deleted_old_objects": result.deleted_old_objects,
    })
}

#[cfg(test)]
mod tests {
    use crate::data::GatewayDataState;
    use crate::state::AppState;

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
}
