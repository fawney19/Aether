use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use aether_data_contracts::repository::background_tasks::*;
use aether_data_query::{
    push_ci_contains, push_eq, push_limit, push_limit_offset, SqlDialect, WhereClause,
};

use crate::error::SqlResultExt;
use crate::{DataLayerError, SqlitePool};

const RUN_COLUMNS: &str = r#"
SELECT
  id,
  task_key,
  kind,
  "trigger",
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
pub struct SqliteBackgroundTaskRepository {
    pool: SqlitePool,
}

impl SqliteBackgroundTaskRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    fn apply_run_filter(builder: &mut QueryBuilder<'_, Sqlite>, query: &BackgroundTaskListQuery) {
        let mut where_clause = WhereClause::new();
        if let Some(kind) = query.kind {
            push_eq(builder, &mut where_clause, "kind", kind.as_database());
        }
        if let Some(status) = query.status {
            push_eq(builder, &mut where_clause, "status", status.as_database());
        }
        if let Some(trigger) = query.trigger.as_deref() {
            push_eq(
                builder,
                &mut where_clause,
                &SqlDialect::Sqlite.quote_ident("trigger"),
                trigger.to_string(),
            );
        }
        if let Some(task_key_substring) = query.task_key_substring.as_deref() {
            push_ci_contains(
                builder,
                &mut where_clause,
                SqlDialect::Sqlite,
                "task_key",
                task_key_substring,
            );
        }
    }
}

#[async_trait]
impl BackgroundTaskReadRepository for SqliteBackgroundTaskRepository {
    async fn find_run(
        &self,
        run_id: &str,
    ) -> Result<Option<StoredBackgroundTaskRun>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(RUN_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(&mut builder, &mut where_clause, "id", run_id.to_string());
        push_limit(&mut builder, 1);
        let row = builder
            .build()
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
            QueryBuilder::<Sqlite>::new("SELECT COUNT(id) AS total FROM background_task_runs");
        Self::apply_run_filter(&mut count_builder, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;

        let mut builder = QueryBuilder::<Sqlite>::new(RUN_COLUMNS);
        Self::apply_run_filter(&mut builder, query);
        builder.push(" ORDER BY created_at_unix_secs DESC, updated_at_unix_secs DESC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(limit, "run limit")?,
            i64_from_usize(query.offset, "run offset")?,
        );
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
        let mut builder = QueryBuilder::<Sqlite>::new(EVENT_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "run_id",
            run_id.to_string(),
        );
        builder.push(" ORDER BY created_at_unix_secs ASC, id ASC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(limit, "event limit")?,
            i64_from_usize(offset, "event offset")?,
        );
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
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
impl BackgroundTaskWriteRepository for SqliteBackgroundTaskRepository {
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
  "trigger",
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
ON CONFLICT(id) DO UPDATE SET
  task_key = excluded.task_key,
  kind = excluded.kind,
  "trigger" = excluded."trigger",
  status = excluded.status,
  attempt = excluded.attempt,
  max_attempts = excluded.max_attempts,
  owner_instance = excluded.owner_instance,
  progress_percent = excluded.progress_percent,
  progress_message = excluded.progress_message,
  payload_json = excluded.payload_json,
  result_json = excluded.result_json,
  error_message = excluded.error_message,
  cancel_requested = excluded.cancel_requested,
  created_by = excluded.created_by,
  created_at_unix_secs = excluded.created_at_unix_secs,
  started_at_unix_secs = excluded.started_at_unix_secs,
  finished_at_unix_secs = excluded.finished_at_unix_secs,
  updated_at_unix_secs = excluded.updated_at_unix_secs
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
        .bind(run.payload_json.as_ref().map(serde_json::Value::to_string))
        .bind(run.result_json.as_ref().map(serde_json::Value::to_string))
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
            "UPDATE background_task_runs SET cancel_requested = 1, updated_at_unix_secs = ? WHERE id = ?",
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
ON CONFLICT(id) DO UPDATE SET
  run_id = excluded.run_id,
  event_type = excluded.event_type,
  message = excluded.message,
  payload_json = excluded.payload_json,
  created_at_unix_secs = excluded.created_at_unix_secs
"#,
        )
        .bind(&event.id)
        .bind(&event.run_id)
        .bind(&event.event_type)
        .bind(&event.message)
        .bind(
            event
                .payload_json
                .as_ref()
                .map(serde_json::Value::to_string),
        )
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

fn map_run_row(row: &SqliteRow) -> Result<StoredBackgroundTaskRun, DataLayerError> {
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
        payload_json: parse_optional_json(row.try_get("payload_json").map_sql_err()?)?,
        result_json: parse_optional_json(row.try_get("result_json").map_sql_err()?)?,
        error_message: row.try_get("error_message").map_sql_err()?,
        cancel_requested: row.try_get("cancel_requested").map_sql_err()?,
        created_by: row.try_get("created_by").map_sql_err()?,
        created_at_unix_secs: u64::try_from(created_at_unix_secs).unwrap_or_default(),
        started_at_unix_secs: started_at_unix_secs.and_then(|value| u64::try_from(value).ok()),
        finished_at_unix_secs: finished_at_unix_secs.and_then(|value| u64::try_from(value).ok()),
        updated_at_unix_secs: u64::try_from(updated_at_unix_secs).unwrap_or_default(),
    })
}

fn map_event_row(row: &SqliteRow) -> Result<StoredBackgroundTaskEvent, DataLayerError> {
    let created_at_unix_secs: i64 = row.try_get("created_at_unix_secs").map_sql_err()?;
    Ok(StoredBackgroundTaskEvent {
        id: row.try_get("id").map_sql_err()?,
        run_id: row.try_get("run_id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        message: row.try_get("message").map_sql_err()?,
        payload_json: parse_optional_json(row.try_get("payload_json").map_sql_err()?)?,
        created_at_unix_secs: u64::try_from(created_at_unix_secs).unwrap_or_default(),
    })
}

fn parse_optional_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|raw| {
            serde_json::from_str::<serde_json::Value>(&raw).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid background task json payload: {err}"
                ))
            })
        })
        .transpose()
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::run_migrations;

    #[tokio::test]
    async fn sqlite_background_task_repository_round_trips_runs_and_events() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let repository = SqliteBackgroundTaskRepository::new(pool);
        let run = repository
            .upsert_run(UpsertBackgroundTaskRun {
                id: "run-1".to_string(),
                task_key: "usage.cleanup".to_string(),
                kind: BackgroundTaskKind::Scheduled,
                trigger: "timer".to_string(),
                status: BackgroundTaskStatus::Queued,
                attempt: 0,
                max_attempts: 3,
                owner_instance: None,
                progress_percent: 0,
                progress_message: None,
                payload_json: Some(json!({"partition": 7})),
                result_json: None,
                error_message: None,
                cancel_requested: false,
                created_by: Some("scheduler".to_string()),
                created_at_unix_secs: 10,
                started_at_unix_secs: None,
                finished_at_unix_secs: None,
                updated_at_unix_secs: 10,
            })
            .await
            .expect("background task run should upsert");
        assert_eq!(run.payload_json, Some(json!({"partition": 7})));

        repository
            .upsert_event(UpsertBackgroundTaskEvent {
                id: "event-1".to_string(),
                run_id: run.id.clone(),
                event_type: "queued".to_string(),
                message: "task queued".to_string(),
                payload_json: Some(json!({"attempt": 0})),
                created_at_unix_secs: 11,
            })
            .await
            .expect("background task event should upsert");

        let page = repository
            .list_runs(&BackgroundTaskListQuery {
                task_key_substring: Some("cleanup".to_string()),
                kind: Some(BackgroundTaskKind::Scheduled),
                status: Some(BackgroundTaskStatus::Queued),
                trigger: Some("timer".to_string()),
                offset: 0,
                limit: 10,
            })
            .await
            .expect("background task runs should list");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, "run-1");

        let events = repository
            .list_events("run-1", 0, 10)
            .await
            .expect("background task events should list");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload_json, Some(json!({"attempt": 0})));

        assert!(repository
            .request_cancel("run-1", 20)
            .await
            .expect("background task cancellation should update"));
        let cancelled = repository
            .find_run("run-1")
            .await
            .expect("background task run should load")
            .expect("background task run should exist");
        assert!(cancelled.cancel_requested);
        assert_eq!(cancelled.updated_at_unix_secs, 20);

        let summary = repository
            .summarize_runs()
            .await
            .expect("background task summary should load");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.by_status.get("queued"), Some(&1));
    }
}
