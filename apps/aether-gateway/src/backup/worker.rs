use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;
use tracing::warn;

use super::config::S3BackupConfig;
use crate::{AppState, GatewayError};

pub(crate) const S3_BACKUP_WORKER_TASK_KEY: &str =
    crate::task_runtime::TASK_KEY_SYSTEM_S3_BACKUP_WORKER;
const S3_BACKUP_WORKER_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) fn should_start_scheduled_backup(last_slot: Option<&str>, current_slot: &str) -> bool {
    last_slot != Some(current_slot)
}

pub(crate) fn spawn_s3_backup_worker(app: AppState) -> Option<JoinHandle<()>> {
    if !app.data.has_system_config_store() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        S3_BACKUP_WORKER_TASK_KEY,
        |app| async move {
            let mut interval = tokio::time::interval(S3_BACKUP_WORKER_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(error) = run_s3_backup_schedule_tick(&app, Utc::now()).await {
                    warn!(error = ?error, "S3 backup schedule tick failed");
                }
            }
        },
    ))
}

async fn run_s3_backup_schedule_tick(
    app: &AppState,
    now: DateTime<Utc>,
) -> Result<(), GatewayError> {
    let values = match super::task::load_s3_backup_config_values(app).await {
        Ok(values) => values,
        Err(error) => {
            warn!(error = %error, "S3 backup schedule config load failed");
            return Ok(());
        }
    };
    let config = match S3BackupConfig::from_json_map(&values) {
        Ok(config) => config,
        Err(error) => {
            warn!(error = %error, "S3 backup schedule config is invalid");
            return Ok(());
        }
    };
    if !config.enabled {
        return Ok(());
    }
    let Some(slot) = config.schedule.due_slot(now) else {
        return Ok(());
    };
    let last_slot = read_last_backup_slot(app).await?;
    if !should_start_scheduled_backup(last_slot.as_deref(), &slot) {
        return Ok(());
    }

    match super::task::start_s3_backup_task_for_schedule(app.clone(), slot).await {
        Ok(_) => {}
        Err(error) => {
            warn!(error = %error, "S3 backup scheduled task submission failed");
        }
    }
    Ok(())
}

async fn read_last_backup_slot(app: &AppState) -> Result<Option<String>, GatewayError> {
    Ok(app
        .read_system_config_json_value(super::S3_BACKUP_LAST_SLOT_KEY)
        .await?
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use crate::backup::schedule::{BackupSchedule, BackupScheduleUnit};
    use crate::task_runtime::{task_definition, TASK_KEY_SYSTEM_S3_BACKUP};

    #[test]
    fn backup_worker_skips_already_recorded_slot() {
        let schedule = BackupSchedule {
            unit: BackupScheduleUnit::Days,
            interval: 1,
            minute: 0,
            hour: 3,
            weekday: 1,
            month_day: 1,
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-24T03:00:30+08:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let slot = schedule.due_slot(now).expect("slot should be due");

        assert!(super::should_start_scheduled_backup(
            Some("days:2026-05-22T19:00:00Z"),
            &slot
        ));
        assert!(!super::should_start_scheduled_backup(Some(&slot), &slot));
    }

    #[test]
    fn backup_worker_has_distinct_supervisor_task_key() {
        assert_ne!(super::S3_BACKUP_WORKER_TASK_KEY, TASK_KEY_SYSTEM_S3_BACKUP);
        assert!(task_definition(super::S3_BACKUP_WORKER_TASK_KEY).is_some());
    }
}
