use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::background_tasks::*;
use aether_data_contracts::DataLayerError;
use aether_data_query::{
    push_ci_contains, push_eq, push_limit, push_limit_offset, SqlDialect, WhereClause,
};

use crate::error::SqlxResultExt;

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
pub struct SqlxBackgroundTaskRepository {
    pool: PgPool,
}

impl SqlxBackgroundTaskRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn apply_run_filter(
        builder: &mut QueryBuilder<'_, Postgres>,
        query: &BackgroundTaskListQuery,
        include_where: bool,
    ) {
        let mut where_clause = if include_where {
            WhereClause::with_existing_clause()
        } else {
            WhereClause::new()
        };
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
                &SqlDialect::Postgres.quote_ident("trigger"),
                trigger.to_string(),
            );
        }
        if let Some(task_key_substring) = query.task_key_substring.as_deref() {
            push_ci_contains(
                builder,
                &mut where_clause,
                SqlDialect::Postgres,
                "task_key",
                task_key_substring,
            );
        }
    }
}

#[async_trait]
impl BackgroundTaskReadRepository for SqlxBackgroundTaskRepository {
    async fn find_run(
        &self,
        run_id: &str,
    ) -> Result<Option<StoredBackgroundTaskRun>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(RUN_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(&mut builder, &mut where_clause, "id", run_id.to_string());
        push_limit(&mut builder, 1);
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_run_row).transpose()
    }

    async fn list_runs(
        &self,
        query: &BackgroundTaskListQuery,
    ) -> Result<StoredBackgroundTaskRunPage, DataLayerError> {
        let limit = query.limit.max(1);

        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(id) AS total FROM background_task_runs");
        Self::apply_run_filter(&mut count_builder, query, false);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;

        let mut builder = QueryBuilder::<Postgres>::new(RUN_COLUMNS);
        Self::apply_run_filter(&mut builder, query, false);
        builder.push(" ORDER BY created_at_unix_secs DESC, updated_at_unix_secs DESC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(limit, "background task run limit")?,
            i64_from_usize(query.offset, "background task run offset")?,
        );
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
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
        let mut builder = QueryBuilder::<Postgres>::new(EVENT_COLUMNS);
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
            i64_from_usize(limit, "background task event limit")?,
            i64_from_usize(offset, "background task event offset")?,
        );
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        rows.iter().map(map_event_row).collect()
    }

    async fn summarize_runs(&self) -> Result<BackgroundTaskSummary, DataLayerError> {
        let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(id) FROM background_task_runs")
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let running_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(id) FROM background_task_runs WHERE status = 'running'",
        )
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        let status_rows = sqlx::query(
            "SELECT status, COUNT(id) AS total FROM background_task_runs GROUP BY status",
        )
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        let kind_rows =
            sqlx::query("SELECT kind, COUNT(id) AS total FROM background_task_runs GROUP BY kind")
                .fetch_all(&self.pool)
                .await
                .map_postgres_err()?;

        let mut by_status = std::collections::BTreeMap::new();
        for row in status_rows {
            let key: String = row.try_get("status").map_postgres_err()?;
            let count: i64 = row.try_get("total").map_postgres_err()?;
            by_status.insert(key, u64::try_from(count).unwrap_or_default());
        }
        let mut by_kind = std::collections::BTreeMap::new();
        for row in kind_rows {
            let key: String = row.try_get("kind").map_postgres_err()?;
            let count: i64 = row.try_get("total").map_postgres_err()?;
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
impl BackgroundTaskWriteRepository for SqlxBackgroundTaskRepository {
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
) VALUES (
  $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19
)
ON CONFLICT(id) DO UPDATE SET
  task_key = EXCLUDED.task_key,
  kind = EXCLUDED.kind,
  "trigger" = EXCLUDED."trigger",
  status = EXCLUDED.status,
  attempt = EXCLUDED.attempt,
  max_attempts = EXCLUDED.max_attempts,
  owner_instance = EXCLUDED.owner_instance,
  progress_percent = EXCLUDED.progress_percent,
  progress_message = EXCLUDED.progress_message,
  payload_json = EXCLUDED.payload_json,
  result_json = EXCLUDED.result_json,
  error_message = EXCLUDED.error_message,
  cancel_requested = EXCLUDED.cancel_requested,
  created_by = EXCLUDED.created_by,
  created_at_unix_secs = EXCLUDED.created_at_unix_secs,
  started_at_unix_secs = EXCLUDED.started_at_unix_secs,
  finished_at_unix_secs = EXCLUDED.finished_at_unix_secs,
  updated_at_unix_secs = EXCLUDED.updated_at_unix_secs
"#,
        )
        .bind(&run.id)
        .bind(&run.task_key)
        .bind(run.kind.as_database())
        .bind(&run.trigger)
        .bind(run.status.as_database())
        .bind(u32_to_i32(run.attempt, "attempt")?)
        .bind(u32_to_i32(run.max_attempts, "max_attempts")?)
        .bind(run.owner_instance.as_deref())
        .bind(i32::from(run.progress_percent))
        .bind(run.progress_message.as_deref())
        .bind(run.payload_json.clone())
        .bind(run.result_json.clone())
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
        .map_postgres_err()?;

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
            "UPDATE background_task_runs SET cancel_requested = TRUE, updated_at_unix_secs = $2 WHERE id = $1",
        )
        .bind(run_id)
        .bind(u64_to_i64(updated_at_unix_secs, "updated_at_unix_secs")?)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
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
  id,
  run_id,
  event_type,
  message,
  payload_json,
  created_at_unix_secs
) VALUES ($1,$2,$3,$4,$5,$6)
ON CONFLICT(id) DO UPDATE SET
  run_id = EXCLUDED.run_id,
  event_type = EXCLUDED.event_type,
  message = EXCLUDED.message,
  payload_json = EXCLUDED.payload_json,
  created_at_unix_secs = EXCLUDED.created_at_unix_secs
"#,
        )
        .bind(&event.id)
        .bind(&event.run_id)
        .bind(&event.event_type)
        .bind(&event.message)
        .bind(event.payload_json.clone())
        .bind(u64_to_i64(
            event.created_at_unix_secs,
            "created_at_unix_secs",
        )?)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;

        let row = sqlx::query(&format!("{EVENT_COLUMNS} WHERE id = $1 LIMIT 1"))
            .bind(&event.id)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        map_event_row(&row)
    }
}

fn map_run_row(row: &PgRow) -> Result<StoredBackgroundTaskRun, DataLayerError> {
    let kind: String = row.try_get("kind").map_postgres_err()?;
    let status: String = row.try_get("status").map_postgres_err()?;
    let attempt: i32 = row.try_get("attempt").map_postgres_err()?;
    let max_attempts: i32 = row.try_get("max_attempts").map_postgres_err()?;
    let progress_percent: i32 = row.try_get("progress_percent").map_postgres_err()?;
    let created_at_unix_secs: i64 = row.try_get("created_at_unix_secs").map_postgres_err()?;
    let started_at_unix_secs: Option<i64> =
        row.try_get("started_at_unix_secs").map_postgres_err()?;
    let finished_at_unix_secs: Option<i64> =
        row.try_get("finished_at_unix_secs").map_postgres_err()?;
    let updated_at_unix_secs: i64 = row.try_get("updated_at_unix_secs").map_postgres_err()?;

    Ok(StoredBackgroundTaskRun {
        id: row.try_get("id").map_postgres_err()?,
        task_key: row.try_get("task_key").map_postgres_err()?,
        kind: BackgroundTaskKind::from_database(&kind)?,
        trigger: row.try_get("trigger").map_postgres_err()?,
        status: BackgroundTaskStatus::from_database(&status)?,
        attempt: u32::try_from(attempt).unwrap_or_default(),
        max_attempts: u32::try_from(max_attempts).unwrap_or_default(),
        owner_instance: row.try_get("owner_instance").map_postgres_err()?,
        progress_percent: u16::try_from(progress_percent).unwrap_or_default(),
        progress_message: row.try_get("progress_message").map_postgres_err()?,
        payload_json: row.try_get("payload_json").map_postgres_err()?,
        result_json: row.try_get("result_json").map_postgres_err()?,
        error_message: row.try_get("error_message").map_postgres_err()?,
        cancel_requested: row.try_get("cancel_requested").map_postgres_err()?,
        created_by: row.try_get("created_by").map_postgres_err()?,
        created_at_unix_secs: u64::try_from(created_at_unix_secs).unwrap_or_default(),
        started_at_unix_secs: started_at_unix_secs.and_then(|value| u64::try_from(value).ok()),
        finished_at_unix_secs: finished_at_unix_secs.and_then(|value| u64::try_from(value).ok()),
        updated_at_unix_secs: u64::try_from(updated_at_unix_secs).unwrap_or_default(),
    })
}

fn map_event_row(row: &PgRow) -> Result<StoredBackgroundTaskEvent, DataLayerError> {
    let created_at_unix_secs: i64 = row.try_get("created_at_unix_secs").map_postgres_err()?;
    Ok(StoredBackgroundTaskEvent {
        id: row.try_get("id").map_postgres_err()?,
        run_id: row.try_get("run_id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        message: row.try_get("message").map_postgres_err()?,
        payload_json: row.try_get("payload_json").map_postgres_err()?,
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

fn u32_to_i32(value: u32, label: &str) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("background task {label} overflow: {value}"))
    })
}
