use std::time::Instant;

use aether_data::repository::system::{AdminSystemPurgeSummary, AdminSystemPurgeTarget};
use aether_data_contracts::DataLayerError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

use super::{now_unix_secs, system_config_usize};

const CLEANUP_RUN_HISTORY_KEY: &str = "admin_cleanup_run_history";
const CLEANUP_RUN_HISTORY_LIMIT: usize = 50;
const REQUEST_BODY_PROGRESS_UPDATE_BATCHES: usize = 10;

const CLEANUP_ERROR_INVALID_CONFIGURATION: &str = "invalid_configuration";
const CLEANUP_ERROR_INVALID_INPUT: &str = "invalid_input";
const CLEANUP_ERROR_POSTGRES: &str = "postgres";
const CLEANUP_ERROR_REDIS: &str = "redis";
const CLEANUP_ERROR_SQL: &str = "sql";
const CLEANUP_ERROR_TIMED_OUT: &str = "timed_out";
const CLEANUP_ERROR_UNEXPECTED_VALUE: &str = "unexpected_value";
const CLEANUP_ERROR_INTERNAL: &str = "internal_error";

pub(crate) const USAGE_CLEANUP_KIND: &str = "usage_cleanup";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AdminCleanupRunRecord {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) trigger: String,
    pub(crate) status: String,
    pub(crate) message: String,
    pub(crate) started_at_unix_secs: u64,
    pub(crate) completed_at_unix_secs: Option<u64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) summary: Value,
    pub(crate) error: Option<String>,
}

pub(super) fn cleanup_data_layer_error_category(error: &DataLayerError) -> &'static str {
    match error {
        DataLayerError::InvalidConfiguration(_) => CLEANUP_ERROR_INVALID_CONFIGURATION,
        DataLayerError::InvalidInput(_) => CLEANUP_ERROR_INVALID_INPUT,
        DataLayerError::Postgres(_) => CLEANUP_ERROR_POSTGRES,
        DataLayerError::Redis(_) => CLEANUP_ERROR_REDIS,
        DataLayerError::Sql(_) => CLEANUP_ERROR_SQL,
        DataLayerError::TimedOut(_) => CLEANUP_ERROR_TIMED_OUT,
        DataLayerError::UnexpectedValue(_) => CLEANUP_ERROR_UNEXPECTED_VALUE,
    }
}

fn cleanup_gateway_error_category(error: &GatewayError) -> &'static str {
    match error {
        GatewayError::UpstreamUnavailable { .. } => "upstream_unavailable",
        GatewayError::ControlUnavailable { .. } => "control_unavailable",
        GatewayError::LocalExecutionPlanningTimeout { .. } => "planning_timed_out",
        GatewayError::AdmissionTimeout { .. } => "admission_timed_out",
        GatewayError::Client { status, .. } if status.is_server_error() => "upstream_error",
        GatewayError::Client { .. } => "request_rejected",
        GatewayError::PlanUsageLimited(_) => "plan_usage_limited",
        GatewayError::LastActiveAdminUpdateDenied | GatewayError::LastActiveAdminDeleteDenied => {
            "operation_rejected"
        }
        GatewayError::Internal(_) => CLEANUP_ERROR_INTERNAL,
    }
}

fn normalize_stored_cleanup_error(error: &str) -> &'static str {
    let error = error.trim();
    match error {
        CLEANUP_ERROR_INVALID_CONFIGURATION => CLEANUP_ERROR_INVALID_CONFIGURATION,
        CLEANUP_ERROR_INVALID_INPUT => CLEANUP_ERROR_INVALID_INPUT,
        CLEANUP_ERROR_POSTGRES => CLEANUP_ERROR_POSTGRES,
        CLEANUP_ERROR_REDIS => CLEANUP_ERROR_REDIS,
        CLEANUP_ERROR_SQL => CLEANUP_ERROR_SQL,
        CLEANUP_ERROR_TIMED_OUT => CLEANUP_ERROR_TIMED_OUT,
        CLEANUP_ERROR_UNEXPECTED_VALUE => CLEANUP_ERROR_UNEXPECTED_VALUE,
        CLEANUP_ERROR_INTERNAL => CLEANUP_ERROR_INTERNAL,
        "upstream_unavailable" => "upstream_unavailable",
        "control_unavailable" => "control_unavailable",
        "planning_timed_out" => "planning_timed_out",
        "admission_timed_out" => "admission_timed_out",
        "upstream_error" => "upstream_error",
        "request_rejected" => "request_rejected",
        "plan_usage_limited" => "plan_usage_limited",
        "operation_rejected" => "operation_rejected",
        _ if error.starts_with("invalid configuration:") => CLEANUP_ERROR_INVALID_CONFIGURATION,
        _ if error.starts_with("invalid input:") => CLEANUP_ERROR_INVALID_INPUT,
        _ if error.starts_with("postgres error:") => CLEANUP_ERROR_POSTGRES,
        _ if error.starts_with("redis error:") => CLEANUP_ERROR_REDIS,
        _ if error.starts_with("sql error:") => CLEANUP_ERROR_SQL,
        _ if error.starts_with("operation timed out:") => CLEANUP_ERROR_TIMED_OUT,
        _ if error.starts_with("unexpected database value:") => CLEANUP_ERROR_UNEXPECTED_VALUE,
        _ => CLEANUP_ERROR_INTERNAL,
    }
}

fn normalize_cleanup_run_record(mut record: AdminCleanupRunRecord) -> AdminCleanupRunRecord {
    record.error = record
        .error
        .as_deref()
        .map(normalize_stored_cleanup_error)
        .map(str::to_string);
    record
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminCleanupTaskKind {
    Config,
    Users,
    RequestBodies,
    Usage,
    AuditLogs,
    Stats,
}

impl AdminCleanupTaskKind {
    fn record_kind(self) -> &'static str {
        match self {
            Self::Config => "config_purge",
            Self::Users => "users_purge",
            Self::RequestBodies => "request_bodies",
            Self::Usage => "usage_purge",
            Self::AuditLogs => "audit_logs_purge",
            Self::Stats => "stats_purge",
        }
    }

    fn target(self) -> Option<AdminSystemPurgeTarget> {
        match self {
            Self::RequestBodies => None,
            Self::Config => Some(AdminSystemPurgeTarget::Config),
            Self::Users => Some(AdminSystemPurgeTarget::Users),
            Self::Usage => Some(AdminSystemPurgeTarget::Usage),
            Self::AuditLogs => Some(AdminSystemPurgeTarget::AuditLogs),
            Self::Stats => Some(AdminSystemPurgeTarget::Stats),
        }
    }

    fn start_message(self, batch_size: Option<usize>) -> String {
        match self {
            Self::Config => "系统配置后台清空已开始".to_string(),
            Self::Users => "非管理员用户后台清空已开始".to_string(),
            Self::RequestBodies => format!(
                "请求/响应体后台清理已开始，每批 {} 条",
                batch_size.unwrap_or(1)
            ),
            Self::Usage => "使用记录后台清空已开始".to_string(),
            Self::AuditLogs => "审计日志后台清空已开始".to_string(),
            Self::Stats => "统计聚合后台清空和重建已开始".to_string(),
        }
    }

    fn failure_message(self) -> &'static str {
        match self {
            Self::Config => "系统配置后台清空失败",
            Self::Users => "非管理员用户后台清空失败",
            Self::RequestBodies => "请求/响应体后台清理失败",
            Self::Usage => "使用记录后台清空失败",
            Self::AuditLogs => "审计日志后台清空失败",
            Self::Stats => "统计聚合后台清空或重建失败",
        }
    }
}

pub(crate) async fn list_admin_cleanup_run_records(
    data: &GatewayDataState,
) -> Result<Vec<AdminCleanupRunRecord>, DataLayerError> {
    let Some(value) = data
        .find_system_config_value(CLEANUP_RUN_HISTORY_KEY)
        .await?
    else {
        return Ok(Vec::new());
    };
    Ok(parse_cleanup_run_records(value))
}

pub(crate) async fn start_admin_request_body_cleanup_task(
    app: AppState,
) -> Result<AdminCleanupRunRecord, GatewayError> {
    start_admin_system_purge_task(app, AdminCleanupTaskKind::RequestBodies).await
}

pub(crate) async fn start_admin_system_purge_task(
    app: AppState,
    kind: AdminCleanupTaskKind,
) -> Result<AdminCleanupRunRecord, GatewayError> {
    let data = app.data.clone();
    let records = list_admin_cleanup_run_records(&data)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    if let Some(existing) = records
        .iter()
        .find(|record| record.kind == kind.record_kind() && record.status == "processing")
        .cloned()
    {
        return Ok(existing);
    }

    let batch_size = if kind == AdminCleanupTaskKind::RequestBodies {
        Some(
            system_config_usize(&data, "cleanup_batch_size", 1_000)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
                .max(1),
        )
    } else {
        None
    };
    let started_at = now_unix_secs();
    let record = AdminCleanupRunRecord {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.record_kind().to_string(),
        trigger: "manual".to_string(),
        status: "processing".to_string(),
        message: kind.start_message(batch_size),
        started_at_unix_secs: started_at,
        completed_at_unix_secs: None,
        duration_ms: None,
        summary: initial_task_summary(kind, batch_size),
        error: None,
    };
    record_cleanup_run(&data, record.clone())
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;

    match kind {
        AdminCleanupTaskKind::RequestBodies => {
            tokio::spawn(run_request_body_cleanup_task(
                data,
                record.clone(),
                batch_size.unwrap_or(1),
            ));
        }
        AdminCleanupTaskKind::Usage
        | AdminCleanupTaskKind::AuditLogs
        | AdminCleanupTaskKind::Config
        | AdminCleanupTaskKind::Users
        | AdminCleanupTaskKind::Stats => {
            tokio::spawn(run_admin_system_purge_task(app, record.clone(), kind));
        }
    }
    Ok(record)
}

pub(crate) async fn record_completed_cleanup_run(
    data: &GatewayDataState,
    kind: &str,
    trigger: &str,
    started_at_unix_secs: u64,
    started_at: Instant,
    summary: Value,
    message: impl Into<String>,
) {
    let record = AdminCleanupRunRecord {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.to_string(),
        trigger: trigger.to_string(),
        status: "completed".to_string(),
        message: message.into(),
        started_at_unix_secs,
        completed_at_unix_secs: Some(now_unix_secs()),
        duration_ms: Some(
            started_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        ),
        summary,
        error: None,
    };
    if let Err(err) = record_cleanup_run(data, record).await {
        warn!(error = %err, kind, "failed to record cleanup run");
    }
}

pub(crate) async fn record_failed_cleanup_run(
    data: &GatewayDataState,
    kind: &str,
    trigger: &str,
    started_at_unix_secs: u64,
    started_at: Instant,
    error: &DataLayerError,
) {
    let record = AdminCleanupRunRecord {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.to_string(),
        trigger: trigger.to_string(),
        status: "failed".to_string(),
        message: "清理执行失败".to_string(),
        started_at_unix_secs,
        completed_at_unix_secs: Some(now_unix_secs()),
        duration_ms: Some(
            started_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        ),
        summary: json!({}),
        error: Some(cleanup_data_layer_error_category(error).to_string()),
    };
    if let Err(err) = record_cleanup_run(data, record).await {
        warn!(error = %err, kind, "failed to record failed cleanup run");
    }
}

async fn run_request_body_cleanup_task(
    data: std::sync::Arc<GatewayDataState>,
    initial_record: AdminCleanupRunRecord,
    batch_size: usize,
) {
    let started_at = Instant::now();
    let mut total = AdminSystemPurgeSummary::default();
    let mut batches = 0usize;

    loop {
        match data.purge_admin_request_bodies_batch(batch_size).await {
            Ok(batch) if batch.total() == 0 => break,
            Ok(batch) => {
                batches = batches.saturating_add(1);
                total.merge(&batch);
                if batches.is_multiple_of(REQUEST_BODY_PROGRESS_UPDATE_BATCHES) {
                    let progress = request_body_cleanup_record(
                        &initial_record,
                        "processing",
                        format!(
                            "请求/响应体后台清理中，已处理 {} 批，影响 {} 行",
                            batches,
                            total.total()
                        ),
                        batches,
                        batch_size,
                        &total,
                        Some(started_at),
                        None,
                    );
                    if let Err(err) = record_cleanup_run(&data, progress).await {
                        warn!(error = %err, "failed to update request body cleanup progress");
                    }
                }
                tokio::task::yield_now().await;
            }
            Err(err) => {
                let failed = request_body_cleanup_record(
                    &initial_record,
                    "failed",
                    "请求/响应体后台清理失败".to_string(),
                    batches,
                    batch_size,
                    &total,
                    Some(started_at),
                    Some(cleanup_data_layer_error_category(&err).to_string()),
                );
                if let Err(record_err) = record_cleanup_run(&data, failed).await {
                    warn!(error = %record_err, "failed to record request body cleanup failure");
                }
                warn!(error = %err, "request body cleanup task failed");
                return;
            }
        }
    }

    let completed = request_body_cleanup_record(
        &initial_record,
        "completed",
        format!(
            "请求/响应体后台清理完成，影响 {} 行，共 {} 批",
            total.total(),
            batches
        ),
        batches,
        batch_size,
        &total,
        Some(started_at),
        None,
    );
    if let Err(err) = record_cleanup_run(&data, completed).await {
        warn!(error = %err, "failed to record request body cleanup completion");
    }
    info!(
        event_name = "request_body_cleanup_task_completed",
        log_type = "ops",
        worker = "request_body_cleanup_task",
        batches,
        affected = total.total(),
        "gateway finished request body cleanup task"
    );
}

async fn run_admin_system_purge_task(
    app: AppState,
    initial_record: AdminCleanupRunRecord,
    kind: AdminCleanupTaskKind,
) {
    let started_at = Instant::now();
    let data = app.data.clone();
    match run_admin_system_purge_task_once(&app, kind).await {
        Ok((summary, message)) => {
            let completed = cleanup_task_record(
                &initial_record,
                "completed",
                message,
                summary,
                Some(started_at),
                None,
            );
            if let Err(err) = record_cleanup_run(&data, completed).await {
                warn!(error = %err, "failed to record admin system purge task completion");
            }
            info!(
                event_name = "admin_system_purge_task_completed",
                log_type = "ops",
                worker = "admin_system_purge_task",
                kind = ?kind,
                "gateway finished admin system purge task"
            );
        }
        Err(err) => {
            let failed = cleanup_task_record(
                &initial_record,
                "failed",
                kind.failure_message().to_string(),
                json!({}),
                Some(started_at),
                Some(cleanup_gateway_error_category(&err).to_string()),
            );
            if let Err(record_err) = record_cleanup_run(&data, failed).await {
                warn!(error = %record_err, "failed to record admin system purge task failure");
            }
            warn!(error = %crate::error::redact_error_debug(&err), kind = ?kind, "admin system purge task failed");
        }
    }
}

async fn run_admin_system_purge_task_once(
    app: &AppState,
    kind: AdminCleanupTaskKind,
) -> Result<(Value, String), GatewayError> {
    let target = kind
        .target()
        .expect("non-request-body cleanup task should map to purge target");
    let purge = app.purge_admin_system_data(target).await?;
    let deleted_total = purge.total();
    let affected = purge.affected.clone();

    if kind == AdminCleanupTaskKind::Stats {
        let rebuild = app.rebuild_admin_stats_once().await?;
        let message = if rebuild.capped {
            format!(
                "统计聚合后台清空完成，已重建 {} 个小时桶和 {} 个日桶，仍有历史统计待后台任务继续重建",
                rebuild.hourly_buckets, rebuild.daily_buckets
            )
        } else {
            format!(
                "统计聚合后台清空并重建完成，删除 {} 行，重建 {} 个小时桶和 {} 个日桶",
                deleted_total, rebuild.hourly_buckets, rebuild.daily_buckets
            )
        };
        return Ok((
            json!({
                "deleted": affected,
                "rebuilt": {
                    "hourly_buckets": rebuild.hourly_buckets,
                    "daily_buckets": rebuild.daily_buckets,
                    "capped": rebuild.capped,
                },
                "total": deleted_total,
            }),
            message,
        ));
    }

    let message = match kind {
        AdminCleanupTaskKind::Config => format!("系统配置后台清空完成，影响 {deleted_total} 行"),
        AdminCleanupTaskKind::Users => format!("非管理员用户后台清空完成，影响 {deleted_total} 行"),
        AdminCleanupTaskKind::Usage => format!("使用记录后台清空完成，影响 {deleted_total} 行"),
        AdminCleanupTaskKind::AuditLogs => format!("审计日志后台清空完成，影响 {deleted_total} 行"),
        AdminCleanupTaskKind::RequestBodies | AdminCleanupTaskKind::Stats => unreachable!(),
    };
    Ok((
        json!({
            "deleted": affected,
            "total": deleted_total,
        }),
        message,
    ))
}

fn initial_task_summary(kind: AdminCleanupTaskKind, batch_size: Option<usize>) -> Value {
    match kind {
        AdminCleanupTaskKind::RequestBodies => json!({
            "batch_size": batch_size.unwrap_or(1),
            "batches": 0,
            "cleaned": {},
        }),
        AdminCleanupTaskKind::Stats => json!({
            "deleted": {},
            "rebuilt": {},
        }),
        AdminCleanupTaskKind::Config
        | AdminCleanupTaskKind::Users
        | AdminCleanupTaskKind::Usage
        | AdminCleanupTaskKind::AuditLogs => json!({
            "deleted": {},
        }),
    }
}

fn cleanup_task_record(
    initial: &AdminCleanupRunRecord,
    status: &str,
    message: String,
    summary: Value,
    started_at: Option<Instant>,
    error: Option<String>,
) -> AdminCleanupRunRecord {
    let completed = matches!(status, "completed" | "failed");
    AdminCleanupRunRecord {
        id: initial.id.clone(),
        kind: initial.kind.clone(),
        trigger: initial.trigger.clone(),
        status: status.to_string(),
        message,
        started_at_unix_secs: initial.started_at_unix_secs,
        completed_at_unix_secs: completed.then(now_unix_secs),
        duration_ms: started_at
            .map(|value| value.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
        summary,
        error,
    }
}

fn request_body_cleanup_record(
    initial: &AdminCleanupRunRecord,
    status: &str,
    message: String,
    batches: usize,
    batch_size: usize,
    total: &AdminSystemPurgeSummary,
    started_at: Option<Instant>,
    error: Option<String>,
) -> AdminCleanupRunRecord {
    let completed = matches!(status, "completed" | "failed");
    AdminCleanupRunRecord {
        id: initial.id.clone(),
        kind: initial.kind.clone(),
        trigger: initial.trigger.clone(),
        status: status.to_string(),
        message,
        started_at_unix_secs: initial.started_at_unix_secs,
        completed_at_unix_secs: completed.then(now_unix_secs),
        duration_ms: started_at
            .map(|value| value.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
        summary: json!({
            "batch_size": batch_size,
            "batches": batches,
            "cleaned": total.affected,
            "total": total.total(),
        }),
        error,
    }
}

pub(crate) async fn record_admin_cleanup_run(
    data: &GatewayDataState,
    record: AdminCleanupRunRecord,
) -> Result<(), DataLayerError> {
    let record = normalize_cleanup_run_record(record);
    let mut records = list_admin_cleanup_run_records(data).await?;
    records.retain(|existing| existing.id != record.id);
    records.insert(0, record);
    records.truncate(CLEANUP_RUN_HISTORY_LIMIT);
    let value = serde_json::to_value(records).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("invalid cleanup run history: {err}"))
    })?;
    data.upsert_system_config_entry(
        CLEANUP_RUN_HISTORY_KEY,
        &value,
        Some("最近的系统清理执行记录"),
    )
    .await?;
    Ok(())
}

async fn record_cleanup_run(
    data: &GatewayDataState,
    record: AdminCleanupRunRecord,
) -> Result<(), DataLayerError> {
    record_admin_cleanup_run(data, record).await
}

fn parse_cleanup_run_records(value: Value) -> Vec<AdminCleanupRunRecord> {
    value
        .as_array()
        .into_iter()
        .flat_map(|items| items.iter())
        .filter_map(|item| serde_json::from_value::<AdminCleanupRunRecord>(item.clone()).ok())
        .map(normalize_cleanup_run_record)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_data_layer_error_category, parse_cleanup_run_records, AdminCleanupRunRecord,
    };
    use aether_data_contracts::DataLayerError;
    use serde_json::json;

    #[test]
    fn cleanup_error_categories_do_not_include_data_layer_details() {
        let error = DataLayerError::Postgres(
            "connection failed for postgresql://admin:database-secret@db.internal/aether"
                .to_string(),
        );

        assert_eq!(cleanup_data_layer_error_category(&error), "postgres");
        assert!(!cleanup_data_layer_error_category(&error).contains("database-secret"));
    }

    #[test]
    fn cleanup_history_read_projection_removes_legacy_error_details() {
        let secret = "postgres error: password=database-secret path=/srv/aether/private.db";
        let stored = AdminCleanupRunRecord {
            id: "cleanup-secret-regression".to_string(),
            kind: "usage_cleanup".to_string(),
            trigger: "manual".to_string(),
            status: "failed".to_string(),
            message: "请求记录手动清理失败".to_string(),
            started_at_unix_secs: 1,
            completed_at_unix_secs: Some(2),
            duration_ms: Some(10),
            summary: json!({}),
            error: Some(secret.to_string()),
        };

        let records = parse_cleanup_run_records(json!([stored]));
        let serialized = serde_json::to_string(&records).expect("cleanup records should serialize");

        assert_eq!(records[0].error.as_deref(), Some("postgres"));
        assert!(!serialized.contains("database-secret"));
        assert!(!serialized.contains("/srv/aether/private.db"));
    }
}
