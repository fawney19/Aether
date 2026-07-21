use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use aether_data_contracts::repository::background_tasks::*;

use crate::error::SqlResultExt;
use crate::{DataLayerError, MysqlPool};

const RUN_COLUMNS: &str = r#"
SELECT
  id,
  task_key,
  kind,
  `trigger`,
  status,
  attempt,
  max_attempts,
  owner_instance,
  progress_percent,
  progress_message,
  payload_json,
  result_json,
  error_message,
  cancel_requested,
  created_by,
  created_at_unix_secs,
  started_at_unix_secs,
  finished_at_unix_secs,
  updated_at_unix_secs
FROM background_task_runs
"#;

const EVENT_COLUMNS: &str = r#"
SELECT
  id,
  run_id,
  event_type,
  message,
  payload_json,
  created_at_unix_secs
FROM background_task_events
"#;

#[derive(Debug, Clone)]
pub struct MysqlBackgroundTaskRepository {
    pool: MysqlPool,
}

impl MysqlBackgroundTaskRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    fn apply_run_filter(builder: &mut QueryBuilder<'_, MySql>, query: &BackgroundTaskListQuery) {
        let mut has_where = false;
        if let Some(kind) = query.kind {
            if !has_where {
                builder.push(" WHERE ");
                has_where = true;
            } else {
                builder.push(" AND ");
            }
            builder.push("kind = ").push_bind(kind.as_database());
        }
        if let Some(status) = query.status {
            if !has_where {
                builder.push(" WHERE ");
                has_where = true;
            } else {
                builder.push(" AND ");
            }
            builder.push("status = ").push_bind(status.as_database());
        }
        if let Some(trigger) = query.trigger.as_deref() {
            if !has_where {
                builder.push(" WHERE ");
                has_where = true;
            } else {
                builder.push(" AND ");
            }
            builder.push("`trigger` = ").push_bind(trigger.to_string());
        }
        if let Some(task_key_substring) = query.task_key_substring.as_deref() {
            if !has_where {
                builder.push(" WHERE ");
            } else {
                builder.push(" AND ");
            }
            builder.push("LOWER(task_key) LIKE ").push_bind(format!(
                "%{}%",
                task_key_substring.trim().to_ascii_lowercase()
            ));
        }
    }
}

#[async_trait]
impl BackgroundTaskReadRepository for MysqlBackgroundTaskRepository {
    async fn find_run(
        &self,
        run_id: &str,
    ) -> Result<Option<StoredBackgroundTaskRun>, DataLayerError> {
        let row = sqlx::query(&format!("{RUN_COLUMNS} WHERE id = ? LIMIT 1"))
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_run_row).transpose()
    }

    async fn list_runs(
        &self,
        query: &BackgroundTaskListQuery,
    ) -> Result<StoredBackgroundTaskRunPage, DataLayerError> {
        let limit = query.limit.max(1);
        let mut count_builder =
            QueryBuilder::<MySql>::new("SELECT COUNT(id) AS total FROM background_task_runs");
        Self::apply_run_filter(&mut count_builder, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;

        let mut builder = QueryBuilder::<MySql>::new(RUN_COLUMNS);
        Self::apply_run_filter(&mut builder, query);
        builder
            .push(" ORDER BY created_at_unix_secs DESC, updated_at_unix_secs DESC")
            .push(" LIMIT ")
            .push_bind(i64_from_usize(limit, "run limit")?)
            .push(" OFFSET ")
            .push_bind(i64_from_usize(query.offset, "run offset")?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let items = rows
            .iter()
            .map(map_run_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredBackgroundTaskRunPage {
            items,
            total: usize::try_from(total).unwrap_or_default(),
        })
    }

    async fn list_events(
        &self,
        run_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredBackgroundTaskEvent>, DataLayerError> {
        let limit = limit.max(1);
        let rows = sqlx::query(&format!(
            "{EVENT_COLUMNS} WHERE run_id = ? ORDER BY created_at_unix_secs ASC, id ASC LIMIT ? OFFSET ?"
        ))
        .bind(run_id)
        .bind(i64_from_usize(limit, "event limit")?)
        .bind(i64_from_usize(offset, "event offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_event_row).collect()
    }

    async fn summarize_runs(&self) -> Result<BackgroundTaskSummary, DataLayerError> {
        let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(id) FROM background_task_runs")
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;
        let running_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(id) FROM background_task_runs WHERE status = 'running'",
        )
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        let status_rows = sqlx::query(
            "SELECT status, COUNT(id) AS total FROM background_task_runs GROUP BY status",
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let kind_rows =
            sqlx::query("SELECT kind, COUNT(id) AS total FROM background_task_runs GROUP BY kind")
                .fetch_all(&self.pool)
                .await
                .map_sql_err()?;

        let mut by_status = std::collections::BTreeMap::new();
        for row in status_rows {
            let key: String = row.try_get("status").map_sql_err()?;
            let count: i64 = row.try_get("total").map_sql_err()?;
            by_status.insert(key, u64::try_from(count).unwrap_or_default());
        }
        let mut by_kind = std::collections::BTreeMap::new();
        for row in kind_rows {
            let key: String = row.try_get("kind").map_sql_err()?;
            let count: i64 = row.try_get("total").map_sql_err()?;
            by_kind.insert(key, u64::try_from(count).unwrap_or_default());
        }

        Ok(BackgroundTaskSummary {
            total: u64::try_from(total).unwrap_or_default(),
            running_count: u64::try_from(running_count).unwrap_or_default(),
            by_status,
            by_kind,
        })
    }
}

#[async_trait]
impl BackgroundTaskWriteRepository for MysqlBackgroundTaskRepository {
    async fn upsert_run(
        &self,
        run: UpsertBackgroundTaskRun,
    ) -> Result<StoredBackgroundTaskRun, DataLayerError> {
        run.validate()?;
        sqlx::query(
            r#"
INSERT INTO background_task_runs (
  id,
  task_key,
  kind,
  `trigger`,
  status,
  attempt,
  max_attempts,
  owner_instance,
  progress_percent,
  progress_message,
  payload_json,
  result_json,
  error_message,
  cancel_requested,
  created_by,
  created_at_unix_secs,
  started_at_unix_secs,
  finished_at_unix_secs,
  updated_at_unix_secs
) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
ON DUPLICATE KEY UPDATE
  task_key = VALUES(task_key),
  kind = VALUES(kind),
  `trigger` = VALUES(`trigger`),
  status = VALUES(status),
  attempt = VALUES(attempt),
  max_attempts = VALUES(max_attempts),
  owner_instance = VALUES(owner_instance),
  progress_percent = VALUES(progress_percent),
  progress_message = VALUES(progress_message),
  payload_json = VALUES(payload_json),
  result_json = VALUES(result_json),
  error_message = VALUES(error_message),
  cancel_requested = VALUES(cancel_requested),
  created_by = VALUES(created_by),
  created_at_unix_secs = VALUES(created_at_unix_secs),
  started_at_unix_secs = VALUES(started_at_unix_secs),
  finished_at_unix_secs = VALUES(finished_at_unix_secs),
  updated_at_unix_secs = VALUES(updated_at_unix_secs)
"#,
        )
        .bind(&run.id)
        .bind(&run.task_key)
        .bind(run.kind.as_database())
        .bind(&run.trigger)
        .bind(run.status.as_database())
        .bind(i64::from(run.attempt))
        .bind(i64::from(run.max_attempts))
        .bind(run.owner_instance.as_deref())
        .bind(i32::from(run.progress_percent))
        .bind(run.progress_message.as_deref())
        .bind(json_to_string(&run.payload_json, "payload_json")?)
        .bind(json_to_string(&run.result_json, "result_json")?)
        .bind(run.error_message.as_deref())
        .bind(run.cancel_requested)
        .bind(run.created_by.as_deref())
        .bind(u64_to_i64(
            run.created_at_unix_secs,
            "created_at_unix_secs",
        )?)
        .bind(run.started_at_unix_secs.map(|value| value as i64))
        .bind(run.finished_at_unix_secs.map(|value| value as i64))
        .bind(u64_to_i64(
            run.updated_at_unix_secs,
            "updated_at_unix_secs",
        )?)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        self.find_run(&run.id).await?.ok_or_else(|| {
            DataLayerError::UnexpectedValue("background task run missing after upsert".to_string())
        })
    }

    async fn request_cancel(
        &self,
        run_id: &str,
        updated_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let affected = sqlx::query(
            "UPDATE background_task_runs SET cancel_requested = TRUE, updated_at_unix_secs = ? WHERE id = ?",
        )
        .bind(u64_to_i64(updated_at_unix_secs, "updated_at_unix_secs")?)
        .bind(run_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(affected > 0)
    }

    async fn upsert_event(
        &self,
        event: UpsertBackgroundTaskEvent,
    ) -> Result<StoredBackgroundTaskEvent, DataLayerError> {
        event.validate()?;
        sqlx::query(
            r#"
INSERT INTO background_task_events (
  id, run_id, event_type, message, payload_json, created_at_unix_secs
) VALUES (?, ?, ?, ?, ?, ?)
ON DUPLICATE KEY UPDATE
  run_id = VALUES(run_id),
  event_type = VALUES(event_type),
  message = VALUES(message),
  payload_json = VALUES(payload_json),
  created_at_unix_secs = VALUES(created_at_unix_secs)
"#,
        )
        .bind(&event.id)
        .bind(&event.run_id)
        .bind(&event.event_type)
        .bind(&event.message)
        .bind(json_to_string(&event.payload_json, "payload_json")?)
        .bind(u64_to_i64(
            event.created_at_unix_secs,
            "created_at_unix_secs",
        )?)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        let row = sqlx::query(&format!("{EVENT_COLUMNS} WHERE id = ? LIMIT 1"))
            .bind(&event.id)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;
        map_event_row(&row)
    }
}

fn map_run_row(row: &MySqlRow) -> Result<StoredBackgroundTaskRun, DataLayerError> {
    let kind: String = row.try_get("kind").map_sql_err()?;
    let status: String = row.try_get("status").map_sql_err()?;
    let attempt: i64 = row.try_get("attempt").map_sql_err()?;
    let max_attempts: i64 = row.try_get("max_attempts").map_sql_err()?;
    let progress_percent: i32 = row.try_get("progress_percent").map_sql_err()?;
    let created_at_unix_secs: i64 = row.try_get("created_at_unix_secs").map_sql_err()?;
    let started_at_unix_secs: Option<i64> = row.try_get("started_at_unix_secs").map_sql_err()?;
    let finished_at_unix_secs: Option<i64> = row.try_get("finished_at_unix_secs").map_sql_err()?;
    let updated_at_unix_secs: i64 = row.try_get("updated_at_unix_secs").map_sql_err()?;

    Ok(StoredBackgroundTaskRun {
        id: row.try_get("id").map_sql_err()?,
        task_key: row.try_get("task_key").map_sql_err()?,
        kind: BackgroundTaskKind::from_database(&kind)?,
        trigger: row.try_get("trigger").map_sql_err()?,
        status: BackgroundTaskStatus::from_database(&status)?,
        attempt: u32::try_from(attempt).unwrap_or_default(),
        max_attempts: u32::try_from(max_attempts).unwrap_or_default(),
        owner_instance: row.try_get("owner_instance").map_sql_err()?,
        progress_percent: u16::try_from(progress_percent).unwrap_or_default(),
        progress_message: row.try_get("progress_message").map_sql_err()?,
        payload_json: parse_optional_json(
            row.try_get("payload_json").ok().flatten(),
            "payload_json",
        )?,
        result_json: parse_optional_json(row.try_get("result_json").ok().flatten(), "result_json")?,
        error_message: row.try_get("error_message").map_sql_err()?,
        cancel_requested: row.try_get("cancel_requested").map_sql_err()?,
        created_by: row.try_get("created_by").map_sql_err()?,
        created_at_unix_secs: u64::try_from(created_at_unix_secs).unwrap_or_default(),
        started_at_unix_secs: started_at_unix_secs.and_then(|value| u64::try_from(value).ok()),
        finished_at_unix_secs: finished_at_unix_secs.and_then(|value| u64::try_from(value).ok()),
        updated_at_unix_secs: u64::try_from(updated_at_unix_secs).unwrap_or_default(),
    })
}

fn map_event_row(row: &MySqlRow) -> Result<StoredBackgroundTaskEvent, DataLayerError> {
    let created_at_unix_secs: i64 = row.try_get("created_at_unix_secs").map_sql_err()?;
    Ok(StoredBackgroundTaskEvent {
        id: row.try_get("id").map_sql_err()?,
        run_id: row.try_get("run_id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        message: row.try_get("message").map_sql_err()?,
        payload_json: parse_optional_json(
            row.try_get("payload_json").ok().flatten(),
            "payload_json",
        )?,
        created_at_unix_secs: u64::try_from(created_at_unix_secs).unwrap_or_default(),
    })
}

fn i64_from_usize(value: usize, label: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("background task {label} overflow: {value}"))
    })
}

fn u64_to_i64(value: u64, label: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("background task {label} overflow: {value}"))
    })
}

fn json_to_string(
    value: &Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .as_ref()
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "background task {field_name} is unserializable: {err}"
                ))
            })
        })
        .transpose()
}

fn parse_optional_json(
    value: Option<String>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "background task {field_name} contains invalid JSON: {err}"
                ))
            })
        })
        .transpose()
}
