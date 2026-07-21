use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskReadRepository, VideoTaskStatus, VideoTaskStatusCount,
    VideoTaskWriteRepository,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::SqlitePool;

const VIDEO_TASK_COLUMNS: &str = r#"
SELECT
  id,
  short_id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  external_task_id,
  provider_id,
  endpoint_id,
  key_id,
  client_api_format,
  provider_api_format,
  format_converted,
  model,
  prompt,
  original_request_body,
  duration_seconds,
  resolution,
  aspect_ratio,
  size,
  status,
  progress_percent,
  progress_message,
  retry_count,
  poll_interval_seconds,
  next_poll_at AS next_poll_at_unix_secs,
  poll_count,
  max_poll_count,
  created_at AS created_at_unix_ms,
  submitted_at AS submitted_at_unix_secs,
  completed_at AS completed_at_unix_secs,
  updated_at AS updated_at_unix_secs,
  error_code,
  error_message,
  video_url,
  request_metadata
FROM video_tasks
"#;

#[derive(Debug, Clone)]
pub struct SqliteVideoTaskRepository {
    pool: SqlitePool,
}

impl SqliteVideoTaskRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn find_by_id(&self, id: &str) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let row = sqlx::query(&format!("{VIDEO_TASK_COLUMNS} WHERE id = ? LIMIT 1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_video_task_row).transpose()
    }

    async fn find_by_short_id(
        &self,
        short_id: &str,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let row = sqlx::query(&format!("{VIDEO_TASK_COLUMNS} WHERE short_id = ? LIMIT 1"))
            .bind(short_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_video_task_row).transpose()
    }

    async fn find_by_user_external(
        &self,
        user_id: &str,
        external_task_id: &str,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let row = sqlx::query(&format!(
            "{VIDEO_TASK_COLUMNS} WHERE user_id = ? AND external_task_id = ? LIMIT 1"
        ))
        .bind(user_id)
        .bind(external_task_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref().map(map_video_task_row).transpose()
    }

    async fn reload_ids(&self, ids: &[String]) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(VIDEO_TASK_COLUMNS);
        builder.push(" WHERE id IN (");
        {
            let mut separated = builder.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
        }
        builder.push(")");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let mut tasks = rows
            .iter()
            .map(map_video_task_row)
            .collect::<Result<Vec<_>, _>>()?;
        tasks.sort_by(|left, right| {
            left.next_poll_at_unix_secs
                .cmp(&right.next_poll_at_unix_secs)
                .then_with(|| left.updated_at_unix_secs.cmp(&right.updated_at_unix_secs))
        });
        Ok(tasks)
    }
}

#[async_trait]
impl VideoTaskReadRepository for SqliteVideoTaskRepository {
    async fn find(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        match key {
            VideoTaskLookupKey::Id(id) => self.find_by_id(id).await,
            VideoTaskLookupKey::ShortId(short_id) => self.find_by_short_id(short_id).await,
            VideoTaskLookupKey::UserExternal {
                user_id,
                external_task_id,
            } => self.find_by_user_external(user_id, external_task_id).await,
        }
    }

    async fn list_active(&self, limit: usize) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(&format!(
            "{VIDEO_TASK_COLUMNS} WHERE status IN ('pending', 'submitted', 'queued', 'processing') ORDER BY updated_at DESC LIMIT ?"
        ))
        .bind(limit_i64(limit, "active video task limit")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_video_task_row).collect()
    }

    async fn list_due(
        &self,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(&format!(
            "{VIDEO_TASK_COLUMNS} WHERE status IN ('submitted', 'queued', 'processing') AND next_poll_at IS NOT NULL AND next_poll_at <= ? AND poll_count < max_poll_count ORDER BY next_poll_at ASC, updated_at ASC LIMIT ?"
        ))
        .bind(u64_to_i64(now_unix_secs, "video task now")?)
        .bind(limit_i64(limit, "due video task limit")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_video_task_row).collect()
    }

    async fn list_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(VIDEO_TASK_COLUMNS);
        push_filter(&mut builder, filter, None);
        builder
            .push(" ORDER BY created_at DESC, updated_at DESC LIMIT ")
            .push_bind(limit_i64(limit, "video task page limit")?)
            .push(" OFFSET ")
            .push_bind(limit_i64(offset, "video task page offset")?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_video_task_row).collect()
    }

    async fn list_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        self.list_page(filter, offset, limit).await
    }

    async fn count(&self, filter: &VideoTaskQueryFilter) -> Result<u64, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT COUNT(id) AS total FROM video_tasks");
        push_filter(&mut builder, filter, None);
        count_query(builder, &self.pool).await
    }

    async fn count_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, DataLayerError> {
        let mut builder =
            QueryBuilder::<Sqlite>::new("SELECT status, COUNT(id) AS total FROM video_tasks");
        push_filter(&mut builder, filter, None);
        builder.push(" GROUP BY status ORDER BY status ASC");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(VideoTaskStatusCount {
                    status: VideoTaskStatus::from_database(
                        row.try_get::<String, _>("status").map_sql_err()?.as_str(),
                    )?,
                    count: count_value(row.try_get("total").map_sql_err()?)?,
                })
            })
            .collect()
    }

    async fn count_distinct_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<Sqlite>::new("SELECT COUNT(DISTINCT user_id) AS total FROM video_tasks");
        push_filter(&mut builder, filter, None);
        push_clause(&mut builder, "user_id IS NOT NULL");
        push_clause(&mut builder, "user_id <> ''");
        count_query(builder, &self.pool).await
    }

    async fn top_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut builder =
            QueryBuilder::<Sqlite>::new("SELECT model, COUNT(id) AS total FROM video_tasks");
        push_filter(&mut builder, filter, None);
        push_clause(&mut builder, "model IS NOT NULL");
        push_clause(&mut builder, "model <> ''");
        builder
            .push(" GROUP BY model ORDER BY total DESC, model ASC LIMIT ")
            .push_bind(limit_i64(limit, "video task top models limit")?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(VideoTaskModelCount {
                    model: row.try_get("model").map_sql_err()?,
                    count: count_value(row.try_get("total").map_sql_err()?)?,
                })
            })
            .collect()
    }

    async fn count_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT COUNT(id) AS total FROM video_tasks");
        push_filter(&mut builder, filter, Some(created_since_unix_secs));
        count_query(builder, &self.pool).await
    }
}

#[async_trait]
impl VideoTaskWriteRepository for SqliteVideoTaskRepository {
    async fn upsert(&self, task: UpsertVideoTask) -> Result<StoredVideoTask, DataLayerError> {
        let id = task.id.clone();
        bind_task(sqlx::query(UPSERT_SQL), task, true, false)?
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        self.find_by_id(&id)
            .await?
            .ok_or_else(|| DataLayerError::UnexpectedValue("upserted video task missing".into()))
    }

    async fn update_if_active(
        &self,
        task: UpsertVideoTask,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let id = task.id.clone();
        let rows_affected = bind_task(sqlx::query(UPDATE_IF_ACTIVE_SQL), task, false, true)?
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        if rows_affected == 0 {
            return Ok(None);
        }
        self.find_by_id(&id).await
    }

    async fn claim_due(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let due = self.list_due(now_unix_secs, limit).await?;
        let ids = due.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
        for id in &ids {
            sqlx::query(
                "UPDATE video_tasks SET next_poll_at = ?, updated_at = MAX(updated_at, ?) WHERE id = ?",
            )
            .bind(u64_to_i64(claim_until_unix_secs, "video task claim_until")?)
            .bind(u64_to_i64(now_unix_secs, "video task now")?)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        }
        self.reload_ids(&ids).await
    }
}

const UPSERT_SQL: &str = r#"
INSERT INTO video_tasks (
  id, short_id, request_id, user_id, api_key_id, username, api_key_name,
  external_task_id, provider_id, endpoint_id, key_id, client_api_format,
  provider_api_format, format_converted, model, prompt, original_request_body,
  duration_seconds, resolution, aspect_ratio, size, status, progress_percent,
  progress_message, retry_count, poll_interval_seconds, next_poll_at, poll_count,
  max_poll_count, video_url, error_code, error_message, request_metadata,
  created_at, submitted_at, completed_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  short_id = excluded.short_id,
  request_id = excluded.request_id,
  user_id = excluded.user_id,
  api_key_id = excluded.api_key_id,
  username = excluded.username,
  api_key_name = excluded.api_key_name,
  external_task_id = excluded.external_task_id,
  provider_id = excluded.provider_id,
  endpoint_id = excluded.endpoint_id,
  key_id = excluded.key_id,
  client_api_format = excluded.client_api_format,
  provider_api_format = excluded.provider_api_format,
  format_converted = excluded.format_converted,
  model = excluded.model,
  prompt = excluded.prompt,
  original_request_body = excluded.original_request_body,
  duration_seconds = excluded.duration_seconds,
  resolution = excluded.resolution,
  aspect_ratio = excluded.aspect_ratio,
  size = excluded.size,
  status = excluded.status,
  progress_percent = excluded.progress_percent,
  progress_message = excluded.progress_message,
  retry_count = excluded.retry_count,
  poll_interval_seconds = excluded.poll_interval_seconds,
  next_poll_at = excluded.next_poll_at,
  poll_count = excluded.poll_count,
  max_poll_count = excluded.max_poll_count,
  video_url = excluded.video_url,
  error_code = excluded.error_code,
  error_message = excluded.error_message,
  request_metadata = excluded.request_metadata,
  created_at = excluded.created_at,
  submitted_at = excluded.submitted_at,
  completed_at = excluded.completed_at,
  updated_at = excluded.updated_at
"#;

const UPDATE_IF_ACTIVE_SQL: &str = r#"
UPDATE video_tasks SET
  short_id = ?,
  request_id = ?,
  user_id = ?,
  api_key_id = ?,
  username = ?,
  api_key_name = ?,
  external_task_id = ?,
  provider_id = ?,
  endpoint_id = ?,
  key_id = ?,
  client_api_format = ?,
  provider_api_format = ?,
  format_converted = ?,
  model = ?,
  prompt = ?,
  original_request_body = ?,
  duration_seconds = ?,
  resolution = ?,
  aspect_ratio = ?,
  size = ?,
  status = ?,
  progress_percent = ?,
  progress_message = ?,
  retry_count = ?,
  poll_interval_seconds = ?,
  next_poll_at = ?,
  poll_count = ?,
  max_poll_count = ?,
  video_url = ?,
  error_code = ?,
  error_message = ?,
  request_metadata = ?,
  created_at = ?,
  submitted_at = ?,
  completed_at = ?,
  updated_at = ?
WHERE id = ?
  AND status IN ('pending', 'submitted', 'queued', 'processing')
"#;

fn bind_task<'q>(
    query: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    task: UpsertVideoTask,
    include_insert_id: bool,
    include_update_id: bool,
) -> Result<sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>, DataLayerError> {
    let original_request_body = json_to_string(&task.original_request_body)?;
    let request_metadata = json_to_string(&task.request_metadata)?;
    let query = if include_insert_id {
        query.bind(task.id.clone())
    } else {
        query
    };
    let bound = query
        .bind(task.short_id)
        .bind(task.request_id)
        .bind(task.user_id)
        .bind(task.api_key_id)
        .bind(task.username)
        .bind(task.api_key_name)
        .bind(task.external_task_id)
        .bind(task.provider_id)
        .bind(task.endpoint_id)
        .bind(task.key_id)
        .bind(task.client_api_format)
        .bind(task.provider_api_format)
        .bind(task.format_converted)
        .bind(task.model)
        .bind(task.prompt)
        .bind(original_request_body)
        .bind(optional_u32_to_i32(
            task.duration_seconds,
            "video task duration_seconds",
        )?)
        .bind(task.resolution)
        .bind(task.aspect_ratio)
        .bind(task.size)
        .bind(status_to_database(task.status))
        .bind(i32::from(task.progress_percent))
        .bind(task.progress_message)
        .bind(u32_to_i32(task.retry_count, "video task retry_count")?)
        .bind(u32_to_i32(
            task.poll_interval_seconds,
            "video task poll_interval_seconds",
        )?)
        .bind(optional_u64_to_i64(
            task.next_poll_at_unix_secs,
            "video task next_poll_at",
        )?)
        .bind(u32_to_i32(task.poll_count, "video task poll_count")?)
        .bind(u32_to_i32(
            task.max_poll_count,
            "video task max_poll_count",
        )?)
        .bind(task.video_url)
        .bind(task.error_code)
        .bind(task.error_message)
        .bind(request_metadata)
        .bind(u64_to_i64(
            task.created_at_unix_ms,
            "video task created_at",
        )?)
        .bind(optional_u64_to_i64(
            task.submitted_at_unix_secs,
            "video task submitted_at",
        )?)
        .bind(optional_u64_to_i64(
            task.completed_at_unix_secs,
            "video task completed_at",
        )?)
        .bind(u64_to_i64(
            task.updated_at_unix_secs,
            "video task updated_at",
        )?);
    if include_update_id {
        Ok(bound.bind(task.id))
    } else {
        Ok(bound)
    }
}

fn push_filter<'args>(
    builder: &mut QueryBuilder<'args, Sqlite>,
    filter: &'args VideoTaskQueryFilter,
    created_since_unix_secs: Option<u64>,
) {
    if let Some(user_id) = filter.user_id.as_deref() {
        push_clause(builder, "user_id = ");
        builder.push_bind(user_id);
    }
    if let Some(status) = filter.status {
        push_clause(builder, "status = ");
        builder.push_bind(status_to_database(status));
    }
    if let Some(model_substring) = filter.model_substring.as_deref() {
        push_clause(builder, "LOWER(model) LIKE ");
        builder.push_bind(format!(
            "%{}%",
            escape_like_pattern(&model_substring.trim().to_ascii_lowercase())
        ));
        builder.push(" ESCAPE '\\'");
    }
    if let Some(client_api_format) = filter.client_api_format.as_deref() {
        push_clause(builder, "client_api_format = ");
        builder.push_bind(client_api_format);
    }
    if let Some(created_since_unix_secs) = created_since_unix_secs {
        push_clause(builder, "created_at >= ");
        builder.push_bind(created_since_unix_secs as i64);
    }
}

fn push_clause<'args>(builder: &mut QueryBuilder<'args, Sqlite>, clause: &str) {
    let sql = builder.sql();
    if sql.contains(" WHERE ") || sql.contains("\nWHERE ") {
        builder.push(" AND ");
    } else {
        builder.push(" WHERE ");
    }
    builder.push(clause);
}

async fn count_query(
    mut builder: QueryBuilder<'_, Sqlite>,
    pool: &SqlitePool,
) -> Result<u64, DataLayerError> {
    let row = builder.build().fetch_one(pool).await.map_sql_err()?;
    count_value(row.try_get("total").map_sql_err()?)
}

fn count_value(value: i64) -> Result<u64, DataLayerError> {
    u64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("invalid video task count result: {value}"))
    })
}

fn map_video_task_row(row: &SqliteRow) -> Result<StoredVideoTask, DataLayerError> {
    StoredVideoTask::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("short_id").map_sql_err()?,
        row.try_get("request_id").map_sql_err()?,
        row.try_get("user_id").map_sql_err()?,
        row.try_get("api_key_id").map_sql_err()?,
        row.try_get("username").map_sql_err()?,
        row.try_get("api_key_name").map_sql_err()?,
        row.try_get("external_task_id").map_sql_err()?,
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("endpoint_id").map_sql_err()?,
        row.try_get("key_id").map_sql_err()?,
        row.try_get("client_api_format").map_sql_err()?,
        row.try_get("provider_api_format").map_sql_err()?,
        row.try_get("format_converted").map_sql_err()?,
        row.try_get("model").map_sql_err()?,
        row.try_get("prompt").map_sql_err()?,
        parse_json(row.try_get("original_request_body").ok().flatten())?,
        row.try_get("duration_seconds").map_sql_err()?,
        row.try_get("resolution").map_sql_err()?,
        row.try_get("aspect_ratio").map_sql_err()?,
        row.try_get("size").map_sql_err()?,
        VideoTaskStatus::from_database(row.try_get::<String, _>("status").map_sql_err()?.as_str())?,
        row.try_get("progress_percent").map_sql_err()?,
        row.try_get("progress_message").map_sql_err()?,
        row.try_get("retry_count").map_sql_err()?,
        row.try_get("poll_interval_seconds").map_sql_err()?,
        row.try_get("next_poll_at_unix_secs").map_sql_err()?,
        row.try_get("poll_count").map_sql_err()?,
        row.try_get("max_poll_count").map_sql_err()?,
        row.try_get("created_at_unix_ms").map_sql_err()?,
        row.try_get("submitted_at_unix_secs").map_sql_err()?,
        row.try_get("completed_at_unix_secs").map_sql_err()?,
        row.try_get("updated_at_unix_secs").map_sql_err()?,
        row.try_get("error_code").map_sql_err()?,
        row.try_get("error_message").map_sql_err()?,
        row.try_get("video_url").map_sql_err()?,
        parse_json(row.try_get("request_metadata").ok().flatten())?,
    )
}

fn parse_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!("video task JSON field is invalid: {err}"))
            })
        })
        .transpose()
}

fn json_to_string(value: &Option<serde_json::Value>) -> Result<Option<String>, DataLayerError> {
    value
        .as_ref()
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "video task JSON field is unserializable: {err}"
                ))
            })
        })
        .transpose()
}

fn status_to_database(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending => "pending",
        VideoTaskStatus::Submitted => "submitted",
        VideoTaskStatus::Queued => "queued",
        VideoTaskStatus::Processing => "processing",
        VideoTaskStatus::Completed => "completed",
        VideoTaskStatus::Failed => "failed",
        VideoTaskStatus::Cancelled => "cancelled",
        VideoTaskStatus::Expired => "expired",
        VideoTaskStatus::Deleted => "deleted",
    }
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn limit_i64(value: usize, name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid {name}: {value}")))
}

fn u64_to_i64(value: u64, name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| DataLayerError::UnexpectedValue(format!("{name} overflow")))
}

fn optional_u64_to_i64(value: Option<u64>, name: &str) -> Result<Option<i64>, DataLayerError> {
    value.map(|value| u64_to_i64(value, name)).transpose()
}

fn u32_to_i32(value: u32, name: &str) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| DataLayerError::UnexpectedValue(format!("{name} overflow")))
}

fn optional_u32_to_i32(value: Option<u32>, name: &str) -> Result<Option<i32>, DataLayerError> {
    value.map(|value| u32_to_i32(value, name)).transpose()
}

#[cfg(test)]
mod tests {
    use super::SqliteVideoTaskRepository;
    use crate::run_migrations;
    use aether_data_contracts::repository::video_tasks::{
        UpsertVideoTask, VideoTaskLookupKey, VideoTaskQueryFilter, VideoTaskReadRepository,
        VideoTaskStatus, VideoTaskWriteRepository,
    };

    #[tokio::test]
    async fn sqlite_repository_writes_and_reads_video_tasks() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let repository = SqliteVideoTaskRepository::new(pool);
        repository
            .upsert(sample_task("task-1", VideoTaskStatus::Submitted, 100))
            .await
            .expect("task should insert");
        repository
            .upsert(UpsertVideoTask {
                user_id: Some("user-2".to_string()),
                model: Some("veo-3-fast".to_string()),
                client_api_format: Some("gemini:video".to_string()),
                created_at_unix_ms: 260,
                updated_at_unix_secs: 260,
                status: VideoTaskStatus::Completed,
                ..sample_task("task-2", VideoTaskStatus::Completed, 260)
            })
            .await
            .expect("task should insert");

        assert!(repository
            .find(VideoTaskLookupKey::ShortId("short-task-1"))
            .await
            .expect("short lookup should load")
            .is_some());
        assert!(repository
            .find(VideoTaskLookupKey::UserExternal {
                user_id: "user-1",
                external_task_id: "ext-task-1",
            })
            .await
            .expect("user/external lookup should load")
            .is_some());

        let due = repository
            .list_due(100, 10)
            .await
            .expect("due tasks should load");
        assert_eq!(due.len(), 1);

        let claimed = repository
            .claim_due(100, 130, 10)
            .await
            .expect("due tasks should claim");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].next_poll_at_unix_secs, Some(130));

        let filter = VideoTaskQueryFilter {
            user_id: Some("user-2".to_string()),
            status: Some(VideoTaskStatus::Completed),
            model_substring: Some("veo".to_string()),
            client_api_format: Some("gemini:video".to_string()),
        };
        assert_eq!(
            repository.count(&filter).await.expect("count should load"),
            1
        );
        assert_eq!(
            repository
                .count_by_status(&filter)
                .await
                .expect("status counts should load")[0]
                .count,
            1
        );
        assert_eq!(
            repository
                .top_models(&filter, 10)
                .await
                .expect("top models should load")[0]
                .model,
            "veo-3-fast"
        );

        let updated = repository
            .update_if_active(UpsertVideoTask {
                status: VideoTaskStatus::Processing,
                progress_percent: 50,
                ..sample_task("task-1", VideoTaskStatus::Processing, 150)
            })
            .await
            .expect("active task should update")
            .expect("active task should exist");
        assert_eq!(updated.progress_percent, 50);
    }

    fn sample_task(
        id: &str,
        status: VideoTaskStatus,
        updated_at_unix_secs: u64,
    ) -> UpsertVideoTask {
        UpsertVideoTask {
            id: id.to_string(),
            short_id: Some(format!("short-{id}")),
            request_id: format!("request-{id}"),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: Some("user".to_string()),
            api_key_name: Some("primary".to_string()),
            external_task_id: Some(format!("ext-{id}")),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("provider-key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("hello".to_string()),
            original_request_body: Some(serde_json::json!({"prompt": "hello"})),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status,
            progress_percent: 0,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(updated_at_unix_secs),
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: updated_at_unix_secs.saturating_sub(10),
            submitted_at_unix_secs: Some(updated_at_unix_secs.saturating_sub(10)),
            completed_at_unix_secs: None,
            updated_at_unix_secs,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: Some(serde_json::json!({"request": id})),
        }
    }
}
