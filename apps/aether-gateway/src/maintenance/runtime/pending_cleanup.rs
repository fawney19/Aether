use std::collections::HashSet;

use chrono::Utc;
use futures_util::TryStreamExt;
use sqlx::Row;

use crate::data::GatewayDataState;
use aether_data_contracts::DataLayerError;

use super::{
    pending_cleanup_batch_size, pending_cleanup_timeout_minutes, postgres_error,
    SELECT_COMPLETED_PENDING_REQUEST_IDS_SQL, SELECT_STALE_PENDING_USAGE_BATCH_SQL,
    UPDATE_FAILED_PENDING_CANDIDATES_SQL, UPDATE_FAILED_STALE_USAGE_SQL,
    UPDATE_FAILED_VOID_STALE_USAGE_SQL, UPDATE_RECOVERED_STALE_USAGE_SQL,
    UPDATE_RECOVERED_STREAMING_CANDIDATES_SQL,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PendingCleanupSummary {
    pub(crate) failed: usize,
    pub(crate) recovered: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StalePendingUsageRow {
    pub(super) id: String,
    pub(super) request_id: String,
    pub(super) status: String,
    pub(super) billing_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FailedPendingUsageRow {
    pub(super) id: String,
    pub(super) error_message: String,
    pub(super) should_void_billing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PendingCleanupBatchPlan {
    pub(super) recovered_usage_ids: Vec<String>,
    pub(super) recovered_request_ids: Vec<String>,
    pub(super) failed_usage_rows: Vec<FailedPendingUsageRow>,
    pub(super) failed_request_ids: Vec<String>,
}

pub(crate) async fn cleanup_stale_pending_requests_once(
    data: &GatewayDataState,
) -> Result<PendingCleanupSummary, DataLayerError> {
    let Some(pool) = data.postgres_pool() else {
        return Ok(PendingCleanupSummary::default());
    };

    let timeout_minutes = pending_cleanup_timeout_minutes(data).await?;
    let batch_size = pending_cleanup_batch_size(data).await?;
    let cutoff_time =
        Utc::now() - chrono::Duration::minutes(i64::try_from(timeout_minutes).unwrap_or(i64::MAX));
    let active_statuses = vec!["pending", "streaming"];
    let mut summary = PendingCleanupSummary::default();

    loop {
        let mut tx = pool.begin().await.map_err(postgres_error)?;
        let stale_rows = {
            let mut stale_rows_stream = sqlx::query(SELECT_STALE_PENDING_USAGE_BATCH_SQL)
                .bind(active_statuses.clone())
                .bind(cutoff_time)
                .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
                .fetch(&mut *tx);
            let mut stale_rows = Vec::new();
            while let Some(row) = stale_rows_stream.try_next().await.map_err(postgres_error)? {
                stale_rows.push(row);
            }
            stale_rows
        };
        if stale_rows.is_empty() {
            tx.rollback().await.map_err(postgres_error)?;
            break;
        }

        let stale_rows = stale_rows
            .into_iter()
            .map(|row| {
                Ok::<StalePendingUsageRow, DataLayerError>(StalePendingUsageRow {
                    id: row.try_get::<String, _>("id").map_err(postgres_error)?,
                    request_id: row
                        .try_get::<String, _>("request_id")
                        .map_err(postgres_error)?,
                    status: row.try_get::<String, _>("status").map_err(postgres_error)?,
                    billing_status: row
                        .try_get::<String, _>("billing_status")
                        .map_err(postgres_error)?,
                })
            })
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        let request_ids = stale_rows
            .iter()
            .map(|row| row.request_id.clone())
            .collect::<Vec<_>>();
        let completed_request_ids = if request_ids.is_empty() {
            HashSet::new()
        } else {
            {
                let mut completed_rows = sqlx::query(SELECT_COMPLETED_PENDING_REQUEST_IDS_SQL)
                    .bind(request_ids)
                    .fetch(&mut *tx);
                let mut completed_request_ids = HashSet::new();
                while let Some(row) = completed_rows.try_next().await.map_err(postgres_error)? {
                    if let Ok(request_id) = row.try_get::<String, _>("request_id") {
                        completed_request_ids.insert(request_id);
                    }
                }
                completed_request_ids
            }
        };
        let plan = plan_pending_cleanup_batch(stale_rows, &completed_request_ids, timeout_minutes);
        let now = Utc::now();

        for usage_id in &plan.recovered_usage_ids {
            sqlx::query(UPDATE_RECOVERED_STALE_USAGE_SQL)
                .bind(usage_id)
                .execute(&mut *tx)
                .await
                .map_err(postgres_error)?;
        }
        for failed_row in &plan.failed_usage_rows {
            if failed_row.should_void_billing {
                sqlx::query(UPDATE_FAILED_VOID_STALE_USAGE_SQL)
                    .bind(&failed_row.id)
                    .bind(&failed_row.error_message)
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .map_err(postgres_error)?;
            } else {
                sqlx::query(UPDATE_FAILED_STALE_USAGE_SQL)
                    .bind(&failed_row.id)
                    .bind(&failed_row.error_message)
                    .execute(&mut *tx)
                    .await
                    .map_err(postgres_error)?;
            }
        }
        if !plan.recovered_request_ids.is_empty() {
            sqlx::query(UPDATE_RECOVERED_STREAMING_CANDIDATES_SQL)
                .bind(plan.recovered_request_ids.clone())
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(postgres_error)?;
        }
        if !plan.failed_request_ids.is_empty() {
            sqlx::query(UPDATE_FAILED_PENDING_CANDIDATES_SQL)
                .bind(plan.failed_request_ids.clone())
                .bind(now)
                .bind(active_statuses.clone())
                .execute(&mut *tx)
                .await
                .map_err(postgres_error)?;
        }

        tx.commit().await.map_err(postgres_error)?;
        summary.failed += plan.failed_usage_rows.len();
        summary.recovered += plan.recovered_usage_ids.len();
    }

    Ok(summary)
}

pub(super) fn plan_pending_cleanup_batch(
    stale_rows: Vec<StalePendingUsageRow>,
    completed_request_ids: &HashSet<String>,
    timeout_minutes: u64,
) -> PendingCleanupBatchPlan {
    let mut plan = PendingCleanupBatchPlan::default();
    for row in stale_rows {
        if completed_request_ids.contains(&row.request_id) {
            plan.recovered_usage_ids.push(row.id);
            plan.recovered_request_ids.push(row.request_id);
            continue;
        }

        plan.failed_request_ids.push(row.request_id);
        plan.failed_usage_rows.push(FailedPendingUsageRow {
            id: row.id,
            error_message: format!(
                "请求超时: 状态 '{}' 超过 {} 分钟未完成",
                row.status, timeout_minutes
            ),
            should_void_billing: row.billing_status == "pending",
        });
    }
    plan
}
