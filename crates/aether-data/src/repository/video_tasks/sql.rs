use async_trait::async_trait;
use futures_util::{stream::TryStream, TryStreamExt};
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use crate::error::SqlxResultExt;
use crate::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskReadRepository, VideoTaskStatus, VideoTaskStatusCount,
    VideoTaskWriteRepository,
};
use crate::DataLayerError;

const SELECT_VIDEO_TASK_COLUMNS_PREFIX: &str = r#"
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
"#;

const SELECT_VIDEO_TASK_COLUMNS_SUFFIX: &str = r#"
  status,
  progress_percent,
  progress_message,
  retry_count,
  poll_interval_seconds,
  CAST(EXTRACT(EPOCH FROM next_poll_at) AS BIGINT) AS next_poll_at_unix_secs,
  poll_count,
  max_poll_count,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM submitted_at) AS BIGINT) AS submitted_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  error_code,
  error_message,
  video_url,
  request_metadata
"#;

fn select_video_task_columns(
    prompt_sql: &str,
    original_request_body_sql: &str,
    duration_seconds_sql: &str,
    resolution_sql: &str,
    aspect_ratio_sql: &str,
    size_sql: &str,
) -> String {
    format!(
        "{SELECT_VIDEO_TASK_COLUMNS_PREFIX}
  {prompt_sql} AS prompt,
  {original_request_body_sql} AS original_request_body,
  {duration_seconds_sql} AS duration_seconds,
  {resolution_sql} AS resolution,
  {aspect_ratio_sql} AS aspect_ratio,
  {size_sql} AS size,
{SELECT_VIDEO_TASK_COLUMNS_SUFFIX}"
    )
}

fn select_video_task_full_columns() -> String {
    select_video_task_columns(
        "prompt",
        "original_request_body",
        "duration_seconds",
        "resolution",
        "aspect_ratio",
        "size",
    )
}

fn select_video_task_claim_columns() -> String {
    select_video_task_columns(
        "NULL::TEXT",
        "NULL::jsonb",
        "NULL::INTEGER",
        "NULL::TEXT",
        "NULL::TEXT",
        "NULL::TEXT",
    )
}

fn select_video_task_sql(where_clause: &str) -> String {
    format!(
        "SELECT\n{}\nFROM video_tasks\n{where_clause}\n",
        select_video_task_full_columns()
    )
}

fn find_by_id_sql() -> String {
    select_video_task_sql("WHERE id = $1\nLIMIT 1")
}

fn find_by_short_id_sql() -> String {
    select_video_task_sql("WHERE short_id = $1\nLIMIT 1")
}

fn find_by_user_external_sql() -> String {
    select_video_task_sql("WHERE user_id = $1 AND external_task_id = $2\nLIMIT 1")
}

fn list_active_sql() -> String {
    select_video_task_sql("WHERE status = ANY($1)\nORDER BY updated_at DESC\nLIMIT $2")
}

fn list_due_sql() -> String {
    select_video_task_sql(
        "WHERE status = ANY($1)\n  AND next_poll_at IS NOT NULL\n  AND next_poll_at <= TO_TIMESTAMP($2)\n  AND poll_count < max_poll_count\nORDER BY next_poll_at ASC\nLIMIT $3",
    )
}

fn select_video_task_page_summary_columns() -> &'static str {
    r#"
  id,
  NULL::TEXT AS short_id,
  request_id,
  user_id,
  NULL::TEXT AS api_key_id,
  username,
  NULL::TEXT AS api_key_name,
  external_task_id,
  provider_id,
  NULL::TEXT AS endpoint_id,
  NULL::TEXT AS key_id,
  NULL::TEXT AS client_api_format,
  NULL::TEXT AS provider_api_format,
  FALSE AS format_converted,
  model,
  CASE
    WHEN prompt IS NULL THEN NULL
    WHEN char_length(prompt) <= 100 THEN prompt
    ELSE LEFT(prompt, 100) || '...'
  END AS prompt,
  NULL::jsonb AS original_request_body,
  duration_seconds,
  resolution,
  aspect_ratio,
  NULL::TEXT AS size,
  status,
  progress_percent,
  progress_message,
  0::INTEGER AS retry_count,
  1::INTEGER AS poll_interval_seconds,
  NULL::BIGINT AS next_poll_at_unix_secs,
  poll_count,
  max_poll_count,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM submitted_at) AS BIGINT) AS submitted_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM completed_at) AS BIGINT) AS completed_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  error_code,
  error_message,
  video_url,
  NULL::jsonb AS request_metadata
"#
}

fn claim_due_sql() -> String {
    let columns = select_video_task_claim_columns();
    format!(
        "WITH due AS (
  SELECT id
  FROM video_tasks
  WHERE status = ANY($1)
    AND next_poll_at IS NOT NULL
    AND next_poll_at <= TO_TIMESTAMP($2)
    AND poll_count < max_poll_count
  ORDER BY next_poll_at ASC, updated_at ASC
  FOR UPDATE SKIP LOCKED
  LIMIT $3
)
UPDATE video_tasks
SET next_poll_at = TO_TIMESTAMP($4),
    updated_at = TO_TIMESTAMP($5)
WHERE id IN (SELECT id FROM due)
RETURNING
{columns}
"
    )
}

fn upsert_sql() -> String {
    let columns = select_video_task_full_columns();
    format!(
        "INSERT INTO video_tasks (
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
  next_poll_at,
  poll_count,
  max_poll_count,
  video_url,
  error_code,
  error_message,
  request_metadata,
  created_at,
  submitted_at,
  completed_at,
  updated_at
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  $14,
  $15,
  $16,
  $17,
  $18,
  $19,
  $20,
  $21,
  $22,
  $23,
  $24,
  $25,
  $26,
  TO_TIMESTAMP($27),
  $28,
  $29,
  $30,
  $31,
  $32,
  $33,
  TO_TIMESTAMP($34),
  TO_TIMESTAMP($35),
  TO_TIMESTAMP($36),
  TO_TIMESTAMP($37)
)
ON CONFLICT (id) DO UPDATE SET
  short_id = EXCLUDED.short_id,
  request_id = EXCLUDED.request_id,
  user_id = EXCLUDED.user_id,
  api_key_id = EXCLUDED.api_key_id,
  username = EXCLUDED.username,
  api_key_name = EXCLUDED.api_key_name,
  external_task_id = EXCLUDED.external_task_id,
  provider_id = EXCLUDED.provider_id,
  endpoint_id = EXCLUDED.endpoint_id,
  key_id = EXCLUDED.key_id,
  client_api_format = EXCLUDED.client_api_format,
  provider_api_format = EXCLUDED.provider_api_format,
  format_converted = EXCLUDED.format_converted,
  model = EXCLUDED.model,
  prompt = EXCLUDED.prompt,
  original_request_body = EXCLUDED.original_request_body,
  duration_seconds = EXCLUDED.duration_seconds,
  resolution = EXCLUDED.resolution,
  aspect_ratio = EXCLUDED.aspect_ratio,
  size = EXCLUDED.size,
  status = EXCLUDED.status,
  progress_percent = EXCLUDED.progress_percent,
  progress_message = EXCLUDED.progress_message,
  retry_count = EXCLUDED.retry_count,
  poll_interval_seconds = EXCLUDED.poll_interval_seconds,
  next_poll_at = EXCLUDED.next_poll_at,
  poll_count = EXCLUDED.poll_count,
  max_poll_count = EXCLUDED.max_poll_count,
  video_url = EXCLUDED.video_url,
  error_code = EXCLUDED.error_code,
  error_message = EXCLUDED.error_message,
  request_metadata = EXCLUDED.request_metadata,
  created_at = EXCLUDED.created_at,
  submitted_at = EXCLUDED.submitted_at,
  completed_at = EXCLUDED.completed_at,
  updated_at = EXCLUDED.updated_at
RETURNING
{columns}
"
    )
}

fn update_if_active_sql() -> String {
    let columns = select_video_task_full_columns();
    format!(
        "UPDATE video_tasks SET
  short_id = $2,
  request_id = $3,
  user_id = $4,
  api_key_id = $5,
  username = $6,
  api_key_name = $7,
  external_task_id = $8,
  provider_id = $9,
  endpoint_id = $10,
  key_id = $11,
  client_api_format = $12,
  provider_api_format = $13,
  format_converted = $14,
  model = $15,
  prompt = $16,
  original_request_body = $17,
  duration_seconds = $18,
  resolution = $19,
  aspect_ratio = $20,
  size = $21,
  status = $22,
  progress_percent = $23,
  progress_message = $24,
  retry_count = $25,
  poll_interval_seconds = $26,
  next_poll_at = TO_TIMESTAMP($27),
  poll_count = $28,
  max_poll_count = $29,
  video_url = $30,
  error_code = $31,
  error_message = $32,
  request_metadata = $33,
  created_at = TO_TIMESTAMP($34),
  submitted_at = TO_TIMESTAMP($35),
  completed_at = TO_TIMESTAMP($36),
  updated_at = TO_TIMESTAMP($37)
WHERE id = $1
  AND status = ANY($38)
RETURNING
{columns}
"
    )
}

#[derive(Debug, Clone)]
pub struct SqlxVideoTaskRepository {
    pool: PgPool,
}

pub type SqlxVideoTaskReadRepository = SqlxVideoTaskRepository;

impl SqlxVideoTaskRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn find(
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

    pub async fn find_by_id(&self, id: &str) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let sql = find_by_id_sql();
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_video_task_row).transpose()
    }

    pub async fn find_by_short_id(
        &self,
        short_id: &str,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let sql = find_by_short_id_sql();
        let row = sqlx::query(&sql)
            .bind(short_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_video_task_row).transpose()
    }

    pub async fn find_by_user_external(
        &self,
        user_id: &str,
        external_task_id: &str,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let sql = find_by_user_external_sql();
        let row = sqlx::query(&sql)
            .bind(user_id)
            .bind(external_task_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_video_task_row).transpose()
    }

    pub async fn list_active(&self, limit: usize) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let active_statuses = vec!["pending", "submitted", "queued", "processing"];
        let sql = list_active_sql();
        collect_query_rows(
            sqlx::query(&sql)
                .bind(active_statuses)
                .bind(i64::try_from(limit).map_err(|_| {
                    DataLayerError::UnexpectedValue(format!("invalid active task limit: {limit}"))
                })?)
                .fetch(&self.pool),
            map_video_task_row,
        )
        .await
    }

    pub async fn list_due(
        &self,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let active_statuses = vec!["submitted", "queued", "processing"];
        let sql = list_due_sql();
        collect_query_rows(
            sqlx::query(&sql)
                .bind(active_statuses)
                .bind(now_unix_secs as f64)
                .bind(i64::try_from(limit).map_err(|_| {
                    DataLayerError::UnexpectedValue(format!("invalid due task limit: {limit}"))
                })?)
                .fetch(&self.pool),
            map_video_task_row,
        )
        .await
    }

    pub async fn list_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let offset = i64::try_from(offset)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid offset: {offset}")))?;
        let limit = i64::try_from(limit)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid limit: {limit}")))?;

        let mut builder = QueryBuilder::<Postgres>::new("SELECT\n");
        builder.push(select_video_task_full_columns());
        builder.push("\nFROM video_tasks");
        push_video_task_filter(&mut builder, filter, None);
        builder.push("\nORDER BY created_at DESC, updated_at DESC");
        builder.push("\nOFFSET ");
        builder.push_bind(offset);
        builder.push("\nLIMIT ");
        builder.push_bind(limit);

        let query = builder.build();
        collect_query_rows(query.fetch(&self.pool), map_video_task_row).await
    }

    pub async fn list_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let offset = i64::try_from(offset)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid offset: {offset}")))?;
        let limit = i64::try_from(limit)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid limit: {limit}")))?;

        let mut builder = QueryBuilder::<Postgres>::new("SELECT\n");
        builder.push(select_video_task_page_summary_columns());
        builder.push("\nFROM video_tasks");
        push_video_task_filter(&mut builder, filter, None);
        builder.push("\nORDER BY created_at DESC, updated_at DESC");
        builder.push("\nOFFSET ");
        builder.push_bind(offset);
        builder.push("\nLIMIT ");
        builder.push_bind(limit);

        let query = builder.build();
        collect_query_rows(query.fetch(&self.pool), map_video_task_row).await
    }

    pub async fn count(&self, filter: &VideoTaskQueryFilter) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(id) AS total FROM video_tasks");
        push_video_task_filter(&mut builder, filter, None);

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let total = row.try_get::<i64, _>("total").map_postgres_err()?;
        u64::try_from(total)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid count result: {total}")))
    }

    pub async fn count_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, DataLayerError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT status, COUNT(id) AS total FROM video_tasks");
        push_video_task_filter(&mut builder, filter, None);
        builder.push("\nGROUP BY status\nORDER BY status ASC");

        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let entry = {
                let status = VideoTaskStatus::from_database(
                    row.try_get::<String, _>("status")
                        .map_postgres_err()?
                        .as_str(),
                )?;
                let total = row.try_get::<i64, _>("total").map_postgres_err()?;
                VideoTaskStatusCount {
                    status,
                    count: u64::try_from(total).map_err(|_| {
                        DataLayerError::UnexpectedValue(format!(
                            "invalid status count result: {total}"
                        ))
                    })?,
                }
            };
            items.push(entry);
        }
        Ok(items)
    }

    pub async fn count_distinct_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(DISTINCT user_id) AS total FROM video_tasks",
        );
        push_video_task_filter(&mut builder, filter, None);
        let mut has_where = has_video_task_filter(filter, None);
        push_sql_clause(&mut builder, &mut has_where, "user_id IS NOT NULL");
        push_sql_clause(&mut builder, &mut has_where, "user_id <> ''");

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let total = row.try_get::<i64, _>("total").map_postgres_err()?;
        u64::try_from(total)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid count result: {total}")))
    }

    pub async fn top_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let limit = i64::try_from(limit)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid limit: {limit}")))?;

        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT model, COUNT(id) AS total FROM video_tasks");
        push_video_task_filter(&mut builder, filter, None);
        let mut has_where = has_video_task_filter(filter, None);
        push_sql_clause(&mut builder, &mut has_where, "model IS NOT NULL");
        push_sql_clause(&mut builder, &mut has_where, "model <> ''");
        builder.push("\nGROUP BY model\nORDER BY total DESC, model ASC\nLIMIT ");
        builder.push_bind(limit);

        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let entry = {
                let model = row.try_get::<String, _>("model").map_postgres_err()?;
                let total = row.try_get::<i64, _>("total").map_postgres_err()?;
                VideoTaskModelCount {
                    model,
                    count: u64::try_from(total).map_err(|_| {
                        DataLayerError::UnexpectedValue(format!(
                            "invalid model count result: {total}"
                        ))
                    })?,
                }
            };
            items.push(entry);
        }
        Ok(items)
    }

    pub async fn count_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(id) AS total FROM video_tasks");
        push_video_task_filter(&mut builder, filter, Some(created_since_unix_secs));

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let total = row.try_get::<i64, _>("total").map_postgres_err()?;
        u64::try_from(total)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid count result: {total}")))
    }

    pub async fn upsert(&self, task: UpsertVideoTask) -> Result<StoredVideoTask, DataLayerError> {
        let sql = upsert_sql();
        let row = sqlx::query(&sql)
            .bind(task.id)
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
            .bind(task.original_request_body)
            .bind(
                task.duration_seconds
                    .map(i32::try_from)
                    .transpose()
                    .map_err(|_| {
                        DataLayerError::UnexpectedValue(
                            "invalid video task duration_seconds".to_string(),
                        )
                    })?,
            )
            .bind(task.resolution)
            .bind(task.aspect_ratio)
            .bind(task.size)
            .bind(map_status_for_database(task.status))
            .bind(i32::from(task.progress_percent))
            .bind(task.progress_message)
            .bind(i32::try_from(task.retry_count).map_err(|_| {
                DataLayerError::UnexpectedValue("invalid video task retry_count".to_string())
            })?)
            .bind(i32::try_from(task.poll_interval_seconds).map_err(|_| {
                DataLayerError::UnexpectedValue(
                    "invalid video task poll_interval_seconds".to_string(),
                )
            })?)
            .bind(task.next_poll_at_unix_secs.map(|value| value as f64))
            .bind(i32::try_from(task.poll_count).map_err(|_| {
                DataLayerError::UnexpectedValue("invalid video task poll_count".to_string())
            })?)
            .bind(i32::try_from(task.max_poll_count).map_err(|_| {
                DataLayerError::UnexpectedValue("invalid video task max_poll_count".to_string())
            })?)
            .bind(task.video_url)
            .bind(task.error_code)
            .bind(task.error_message)
            .bind(task.request_metadata)
            .bind(task.created_at_unix_ms as f64)
            .bind(task.submitted_at_unix_secs.map(|value| value as f64))
            .bind(task.completed_at_unix_secs.map(|value| value as f64))
            .bind(task.updated_at_unix_secs as f64)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;

        map_video_task_row(&row)
    }

    pub async fn update_if_active(
        &self,
        task: UpsertVideoTask,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let sql = update_if_active_sql();
        let row = sqlx::query(&sql)
            .bind(task.id)
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
            .bind(task.original_request_body)
            .bind(
                task.duration_seconds
                    .map(i32::try_from)
                    .transpose()
                    .map_err(|_| {
                        DataLayerError::UnexpectedValue(
                            "invalid video task duration_seconds".to_string(),
                        )
                    })?,
            )
            .bind(task.resolution)
            .bind(task.aspect_ratio)
            .bind(task.size)
            .bind(map_status_for_database(task.status))
            .bind(i32::from(task.progress_percent))
            .bind(task.progress_message)
            .bind(i32::try_from(task.retry_count).map_err(|_| {
                DataLayerError::UnexpectedValue("invalid video task retry_count".to_string())
            })?)
            .bind(i32::try_from(task.poll_interval_seconds).map_err(|_| {
                DataLayerError::UnexpectedValue(
                    "invalid video task poll_interval_seconds".to_string(),
                )
            })?)
            .bind(task.next_poll_at_unix_secs.map(|value| value as f64))
            .bind(i32::try_from(task.poll_count).map_err(|_| {
                DataLayerError::UnexpectedValue("invalid video task poll_count".to_string())
            })?)
            .bind(i32::try_from(task.max_poll_count).map_err(|_| {
                DataLayerError::UnexpectedValue("invalid video task max_poll_count".to_string())
            })?)
            .bind(task.video_url)
            .bind(task.error_code)
            .bind(task.error_message)
            .bind(task.request_metadata)
            .bind(task.created_at_unix_ms as f64)
            .bind(task.submitted_at_unix_secs.map(|value| value as f64))
            .bind(task.completed_at_unix_secs.map(|value| value as f64))
            .bind(task.updated_at_unix_secs as f64)
            .bind(vec!["pending", "submitted", "queued", "processing"])
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;

        row.as_ref().map(map_video_task_row).transpose()
    }

    pub async fn claim_due(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let limit = i64::try_from(limit)
            .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid limit: {limit}")))?;
        let sql = claim_due_sql();
        let mut tasks = collect_query_rows(
            sqlx::query(&sql)
                .bind(vec!["submitted", "queued", "processing"])
                .bind(now_unix_secs as f64)
                .bind(limit)
                .bind(claim_until_unix_secs as f64)
                .bind(now_unix_secs as f64)
                .fetch(&self.pool),
            map_video_task_row,
        )
        .await?;
        tasks.sort_by(|left, right| {
            left.next_poll_at_unix_secs
                .cmp(&right.next_poll_at_unix_secs)
                .then_with(|| left.updated_at_unix_secs.cmp(&right.updated_at_unix_secs))
        });
        Ok(tasks)
    }
}

#[async_trait]
impl VideoTaskReadRepository for SqlxVideoTaskRepository {
    async fn find(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        Self::find(self, key).await
    }

    async fn list_active(&self, limit: usize) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        Self::list_active(self, limit).await
    }

    async fn list_due(
        &self,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        Self::list_due(self, now_unix_secs, limit).await
    }

    async fn list_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        Self::list_page(self, filter, offset, limit).await
    }

    async fn list_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        Self::list_page_summary(self, filter, offset, limit).await
    }

    async fn count(&self, filter: &VideoTaskQueryFilter) -> Result<u64, DataLayerError> {
        Self::count(self, filter).await
    }

    async fn count_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, DataLayerError> {
        Self::count_by_status(self, filter).await
    }

    async fn count_distinct_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, DataLayerError> {
        Self::count_distinct_users(self, filter).await
    }

    async fn top_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, DataLayerError> {
        Self::top_models(self, filter, limit).await
    }

    async fn count_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        Self::count_created_since(self, filter, created_since_unix_secs).await
    }
}

#[async_trait]
impl VideoTaskWriteRepository for SqlxVideoTaskRepository {
    async fn upsert(&self, task: UpsertVideoTask) -> Result<StoredVideoTask, DataLayerError> {
        Self::upsert(self, task).await
    }

    async fn update_if_active(
        &self,
        task: UpsertVideoTask,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        Self::update_if_active(self, task).await
    }

    async fn claim_due(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        Self::claim_due(self, now_unix_secs, claim_until_unix_secs, limit).await
    }
}

fn has_video_task_filter(
    filter: &VideoTaskQueryFilter,
    created_since_unix_secs: Option<u64>,
) -> bool {
    filter.user_id.is_some()
        || filter.status.is_some()
        || filter.model_substring.is_some()
        || filter.client_api_format.is_some()
        || created_since_unix_secs.is_some()
}

fn push_sql_clause<'args>(
    builder: &mut QueryBuilder<'args, Postgres>,
    has_where: &mut bool,
    clause: &str,
) {
    if *has_where {
        builder.push("\n  AND ");
    } else {
        builder.push("\nWHERE ");
        *has_where = true;
    }
    builder.push(clause);
}

fn push_video_task_filter<'args>(
    builder: &mut QueryBuilder<'args, Postgres>,
    filter: &'args VideoTaskQueryFilter,
    created_since_unix_secs: Option<u64>,
) {
    let mut has_where = false;

    if let Some(user_id) = filter.user_id.as_deref() {
        push_sql_clause(builder, &mut has_where, "user_id = ");
        builder.push_bind(user_id);
    }
    if let Some(status) = filter.status {
        push_sql_clause(builder, &mut has_where, "status = ");
        builder.push_bind(map_status_for_database(status));
    }
    if let Some(model_substring) = filter.model_substring.as_deref() {
        push_sql_clause(builder, &mut has_where, "model ILIKE ");
        builder.push_bind(format!("%{}%", escape_like_pattern(model_substring.trim())));
        builder.push(" ESCAPE '\\'");
    }
    if let Some(client_api_format) = filter.client_api_format.as_deref() {
        push_sql_clause(builder, &mut has_where, "client_api_format = ");
        builder.push_bind(client_api_format);
    }
    if let Some(created_since_unix_secs) = created_since_unix_secs {
        push_sql_clause(builder, &mut has_where, "created_at >= TO_TIMESTAMP(");
        builder.push_bind(created_since_unix_secs as f64);
        builder.push(")");
    }
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn map_status_for_database(status: VideoTaskStatus) -> &'static str {
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

async fn collect_query_rows<T, S>(
    mut rows: S,
    map_row: fn(&PgRow) -> Result<T, DataLayerError>,
) -> Result<Vec<T>, DataLayerError>
where
    S: TryStream<Ok = PgRow, Error = sqlx::Error> + Unpin,
{
    let mut items = Vec::new();
    while let Some(row) = rows.try_next().await.map_postgres_err()? {
        items.push(map_row(&row)?);
    }
    Ok(items)
}

fn map_video_task_row(row: &PgRow) -> Result<StoredVideoTask, DataLayerError> {
    let status = VideoTaskStatus::from_database(
        row.try_get::<String, _>("status")
            .map_postgres_err()?
            .as_str(),
    )?;
    StoredVideoTask::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("short_id").map_postgres_err()?,
        row.try_get("request_id").map_postgres_err()?,
        row.try_get("user_id").map_postgres_err()?,
        row.try_get("api_key_id").map_postgres_err()?,
        row.try_get("username").map_postgres_err()?,
        row.try_get("api_key_name").map_postgres_err()?,
        row.try_get("external_task_id").map_postgres_err()?,
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("endpoint_id").map_postgres_err()?,
        row.try_get("key_id").map_postgres_err()?,
        row.try_get("client_api_format").map_postgres_err()?,
        row.try_get("provider_api_format").map_postgres_err()?,
        row.try_get("format_converted").map_postgres_err()?,
        row.try_get("model").map_postgres_err()?,
        row.try_get("prompt").map_postgres_err()?,
        row.try_get("original_request_body").map_postgres_err()?,
        row.try_get("duration_seconds").map_postgres_err()?,
        row.try_get("resolution").map_postgres_err()?,
        row.try_get("aspect_ratio").map_postgres_err()?,
        row.try_get("size").map_postgres_err()?,
        status,
        row.try_get("progress_percent").map_postgres_err()?,
        row.try_get("progress_message").map_postgres_err()?,
        row.try_get("retry_count").map_postgres_err()?,
        row.try_get("poll_interval_seconds").map_postgres_err()?,
        row.try_get("next_poll_at_unix_secs").map_postgres_err()?,
        row.try_get("poll_count").map_postgres_err()?,
        row.try_get("max_poll_count").map_postgres_err()?,
        row.try_get("created_at_unix_ms").map_postgres_err()?,
        row.try_get("submitted_at_unix_secs").map_postgres_err()?,
        row.try_get("completed_at_unix_secs").map_postgres_err()?,
        row.try_get("updated_at_unix_secs").map_postgres_err()?,
        row.try_get("error_code").map_postgres_err()?,
        row.try_get("error_message").map_postgres_err()?,
        row.try_get("video_url").map_postgres_err()?,
        row.try_get("request_metadata").map_postgres_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqlxVideoTaskRepository;
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};
    use crate::repository::video_tasks::{
        UpsertVideoTask, VideoTaskLookupKey, VideoTaskQueryFilter, VideoTaskReadRepository,
        VideoTaskStatus, VideoTaskWriteRepository,
    };

    fn build_pool() -> sqlx::PgPool {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        factory.connect_lazy().expect("pool should build")
    }

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
        let repository = SqlxVideoTaskRepository::new(build_pool());
        let _ = repository.pool();
    }

    #[tokio::test]
    async fn read_trait_delegates_to_sqlx_repository() {
        let repository = SqlxVideoTaskRepository::new(build_pool());
        let _ = VideoTaskReadRepository::find(&repository, VideoTaskLookupKey::Id("task-1")).await;
    }

    #[tokio::test]
    async fn read_trait_delegates_due_listing_to_sqlx_repository() {
        let repository = SqlxVideoTaskRepository::new(build_pool());
        let _ = VideoTaskReadRepository::list_due(&repository, 1, 10).await;
    }

    #[tokio::test]
    async fn read_trait_delegates_query_methods_to_sqlx_repository() {
        let repository = SqlxVideoTaskRepository::new(build_pool());
        let filter = VideoTaskQueryFilter::default();
        let _ = VideoTaskReadRepository::list_page(&repository, &filter, 0, 10).await;
        let _ = VideoTaskReadRepository::count(&repository, &filter).await;
        let _ = VideoTaskReadRepository::count_by_status(&repository, &filter).await;
        let _ = VideoTaskReadRepository::top_models(&repository, &filter, 10).await;
        let _ = VideoTaskReadRepository::count_created_since(&repository, &filter, 0).await;
    }

    #[tokio::test]
    async fn write_trait_delegates_to_sqlx_repository() {
        let repository = SqlxVideoTaskRepository::new(build_pool());
        let _ = VideoTaskWriteRepository::upsert(
            &repository,
            UpsertVideoTask {
                id: "task-1".to_string(),
                short_id: Some("short-task-1".to_string()),
                request_id: "request-1".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                external_task_id: Some("ext-1".to_string()),
                provider_id: Some("provider-1".to_string()),
                endpoint_id: Some("endpoint-1".to_string()),
                key_id: Some("key-1".to_string()),
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
                status: VideoTaskStatus::Submitted,
                progress_percent: 0,
                progress_message: None,
                retry_count: 0,
                poll_interval_seconds: 10,
                next_poll_at_unix_secs: Some(10),
                poll_count: 0,
                max_poll_count: 360,
                created_at_unix_ms: 1,
                submitted_at_unix_secs: Some(1),
                completed_at_unix_secs: None,
                updated_at_unix_secs: 1,
                error_code: None,
                error_message: None,
                video_url: None,
                request_metadata: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn write_trait_delegates_active_update_to_sqlx_repository() {
        let repository = SqlxVideoTaskRepository::new(build_pool());
        let _ = VideoTaskWriteRepository::update_if_active(
            &repository,
            UpsertVideoTask {
                id: "task-1".to_string(),
                short_id: Some("short-task-1".to_string()),
                request_id: "request-1".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                external_task_id: Some("ext-1".to_string()),
                provider_id: Some("provider-1".to_string()),
                endpoint_id: Some("endpoint-1".to_string()),
                key_id: Some("key-1".to_string()),
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
                status: VideoTaskStatus::Processing,
                progress_percent: 50,
                progress_message: Some("polling".to_string()),
                retry_count: 1,
                poll_interval_seconds: 10,
                next_poll_at_unix_secs: Some(20),
                poll_count: 1,
                max_poll_count: 360,
                created_at_unix_ms: 1,
                submitted_at_unix_secs: Some(1),
                completed_at_unix_secs: None,
                updated_at_unix_secs: 20,
                error_code: None,
                error_message: None,
                video_url: None,
                request_metadata: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn write_trait_delegates_due_claim_to_sqlx_repository() {
        let repository = SqlxVideoTaskRepository::new(build_pool());
        let _ = VideoTaskWriteRepository::claim_due(&repository, 1, 30, 10).await;
    }
}
