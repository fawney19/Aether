use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use super::{
    PublicHealthStatusCount, PublicHealthTimelineBucket, RequestCandidateReadRepository,
    RequestCandidateStatus, RequestCandidateWriteRepository, StoredRequestCandidate,
    UpsertRequestCandidateRecord,
};
use crate::driver::mysql::MysqlPool;
use crate::error::SqlResultExt;
use crate::DataLayerError;

const CANDIDATE_COLUMNS: &str = r#"
SELECT
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  candidate_index,
  retry_index,
  provider_id,
  endpoint_id,
  key_id,
  status,
  skip_reason,
  is_cached,
  status_code,
  error_type,
  error_message,
  latency_ms,
  concurrent_requests,
  extra_data,
  required_capabilities,
  created_at AS created_at_unix_ms,
  started_at AS started_at_unix_ms,
  finished_at AS finished_at_unix_ms
FROM request_candidates
"#;

#[derive(Debug, Clone)]
pub struct MysqlRequestCandidateRepository {
    pool: MysqlPool,
}

impl MysqlRequestCandidateRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn find_by_unique(
        &self,
        request_id: &str,
        candidate_index: u32,
        retry_index: u32,
    ) -> Result<Option<StoredRequestCandidate>, DataLayerError> {
        let row = sqlx::query(&format!(
            "{CANDIDATE_COLUMNS} WHERE request_id = ? AND candidate_index = ? AND retry_index = ? LIMIT 1"
        ))
        .bind(request_id)
        .bind(to_i32(candidate_index)?)
        .bind(to_i32(retry_index)?)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref().map(map_candidate_row).transpose()
    }
}

#[async_trait]
impl RequestCandidateReadRepository for MysqlRequestCandidateRepository {
    async fn list_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        let rows = sqlx::query(&format!(
            "{CANDIDATE_COLUMNS} WHERE request_id = ? ORDER BY candidate_index ASC, retry_index ASC, created_at ASC"
        ))
        .bind(request_id)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_candidate_row).collect()
    }

    async fn list_attempted_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        let rows = sqlx::query(&format!(
            "{CANDIDATE_COLUMNS} WHERE request_id = ? \
             AND (status IN ('streaming', 'success', 'failed', 'cancelled') \
             OR (status = 'pending' AND started_at IS NOT NULL)) \
             ORDER BY candidate_index ASC, retry_index ASC, created_at ASC"
        ))
        .bind(request_id)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_candidate_row).collect()
    }

    async fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(&format!(
            "{CANDIDATE_COLUMNS} ORDER BY created_at DESC LIMIT ?"
        ))
        .bind(limit_i64(limit, "recent request candidate limit")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_candidate_row).collect()
    }

    async fn list_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(&format!(
            "{CANDIDATE_COLUMNS} WHERE provider_id = ? ORDER BY created_at DESC LIMIT ?"
        ))
        .bind(provider_id)
        .bind(limit_i64(limit, "provider request candidate limit")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_candidate_row).collect()
    }

    async fn list_finalized_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if endpoint_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(CANDIDATE_COLUMNS);
        push_endpoint_in_clause(&mut builder, endpoint_ids);
        builder
            .push(" AND created_at >= ")
            .push_bind(unix_secs_to_ms_i64(since_unix_secs)?)
            .push(" AND status IN ('success', 'failed', 'skipped')")
            .push(" ORDER BY created_at DESC LIMIT ")
            .push_bind(limit_i64(limit, "finalized request candidate limit")?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_candidate_row).collect()
    }

    async fn count_finalized_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<PublicHealthStatusCount>, DataLayerError> {
        if endpoint_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(
            "SELECT endpoint_id, status, COUNT(id) AS count FROM request_candidates",
        );
        push_endpoint_in_clause(&mut builder, endpoint_ids);
        builder
            .push(" AND created_at >= ")
            .push_bind(unix_secs_to_ms_i64(since_unix_secs)?)
            .push(" AND status IN ('success', 'failed', 'skipped')")
            .push(" GROUP BY endpoint_id, status");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(PublicHealthStatusCount {
                    endpoint_id: row.try_get("endpoint_id").map_sql_err()?,
                    status: RequestCandidateStatus::from_database(
                        row.try_get::<String, _>("status").map_sql_err()?.as_str(),
                    )?,
                    count: u64::try_from(row.try_get::<i64, _>("count").map_sql_err()?).map_err(
                        |_| {
                            DataLayerError::UnexpectedValue(
                                "public health status count out of range".to_string(),
                            )
                        },
                    )?,
                })
            })
            .collect()
    }

    async fn aggregate_finalized_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<Vec<PublicHealthTimelineBucket>, DataLayerError> {
        if endpoint_ids.is_empty() || segments == 0 || until_unix_secs < since_unix_secs {
            return Ok(Vec::new());
        }
        let since_ms = unix_secs_to_ms_i64(since_unix_secs)?;
        let until_ms = unix_secs_to_ms_i64(until_unix_secs)?;
        let mut builder = QueryBuilder::<MySql>::new(CANDIDATE_COLUMNS);
        push_endpoint_in_clause(&mut builder, endpoint_ids);
        builder
            .push(" AND created_at >= ")
            .push_bind(since_ms)
            .push(" AND created_at <= ")
            .push_bind(until_ms)
            .push(" AND status IN ('success', 'failed', 'skipped')");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        aggregate_timeline(
            rows.iter()
                .map(map_candidate_row)
                .collect::<Result<Vec<_>, _>>()?,
            since_unix_secs,
            until_unix_secs,
            segments,
        )
    }
}

#[async_trait]
impl RequestCandidateWriteRepository for MysqlRequestCandidateRepository {
    async fn upsert(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<StoredRequestCandidate, DataLayerError> {
        candidate.validate()?;
        let existing = self
            .find_by_unique(
                &candidate.request_id,
                candidate.candidate_index,
                candidate.retry_index,
            )
            .await?;
        let merged = merge_candidate(candidate, existing)?;
        upsert_merged_candidate(&self.pool, &merged).await?;
        Ok(merged)
    }

    async fn delete_created_before(
        &self,
        created_before_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        if limit == 0 {
            return Ok(0);
        }
        let rows_affected = sqlx::query(
            r#"
DELETE FROM request_candidates
WHERE id IN (
  SELECT id
  FROM (
    SELECT id
    FROM request_candidates
    WHERE created_at < ?
    ORDER BY created_at ASC, id ASC
    LIMIT ?
  ) AS old_request_candidates
)
"#,
        )
        .bind(unix_secs_to_ms_i64(created_before_unix_secs)?)
        .bind(limit_i64(limit, "request candidate delete limit")?)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(usize::try_from(rows_affected).unwrap_or_default())
    }
}

async fn upsert_merged_candidate(
    pool: &MysqlPool,
    candidate: &StoredRequestCandidate,
) -> Result<(), DataLayerError> {
    sqlx::query(
        r#"
INSERT INTO request_candidates (
  id, request_id, user_id, api_key_id, username, api_key_name,
  candidate_index, retry_index, provider_id, endpoint_id, key_id, status,
  skip_reason, is_cached, status_code, error_type, error_message, latency_ms,
  concurrent_requests, extra_data, required_capabilities, created_at, started_at, finished_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON DUPLICATE KEY UPDATE
  user_id = VALUES(user_id),
  api_key_id = VALUES(api_key_id),
  username = VALUES(username),
  api_key_name = VALUES(api_key_name),
  provider_id = VALUES(provider_id),
  endpoint_id = VALUES(endpoint_id),
  key_id = VALUES(key_id),
  status = VALUES(status),
  skip_reason = VALUES(skip_reason),
  is_cached = VALUES(is_cached),
  status_code = VALUES(status_code),
  error_type = VALUES(error_type),
  error_message = VALUES(error_message),
  latency_ms = VALUES(latency_ms),
  concurrent_requests = VALUES(concurrent_requests),
  extra_data = VALUES(extra_data),
  required_capabilities = VALUES(required_capabilities),
  created_at = VALUES(created_at),
  started_at = VALUES(started_at),
  finished_at = VALUES(finished_at)
"#,
    )
    .bind(&candidate.id)
    .bind(&candidate.request_id)
    .bind(&candidate.user_id)
    .bind(&candidate.api_key_id)
    .bind(&candidate.username)
    .bind(&candidate.api_key_name)
    .bind(to_i32(candidate.candidate_index)?)
    .bind(to_i32(candidate.retry_index)?)
    .bind(&candidate.provider_id)
    .bind(&candidate.endpoint_id)
    .bind(&candidate.key_id)
    .bind(status_to_database(candidate.status))
    .bind(&candidate.skip_reason)
    .bind(candidate.is_cached)
    .bind(candidate.status_code.map(i32::from))
    .bind(&candidate.error_type)
    .bind(&candidate.error_message)
    .bind(candidate.latency_ms.map(to_i32_u64).transpose()?)
    .bind(candidate.concurrent_requests.map(to_i32).transpose()?)
    .bind(json_to_string(&candidate.extra_data)?)
    .bind(json_to_string(&candidate.required_capabilities)?)
    .bind(u64_to_i64(
        candidate.created_at_unix_ms,
        "request candidate created_at",
    )?)
    .bind(optional_u64_to_i64(
        candidate.started_at_unix_ms,
        "request candidate started_at",
    )?)
    .bind(optional_u64_to_i64(
        candidate.finished_at_unix_ms,
        "request candidate finished_at",
    )?)
    .execute(pool)
    .await
    .map_sql_err()?;
    Ok(())
}

fn push_endpoint_in_clause<'args>(
    builder: &mut QueryBuilder<'args, MySql>,
    endpoint_ids: &'args [String],
) {
    builder.push(" WHERE endpoint_id IN (");
    {
        let mut separated = builder.separated(", ");
        for endpoint_id in endpoint_ids {
            separated.push_bind(endpoint_id);
        }
    }
    builder.push(")");
}

fn merge_candidate(
    candidate: UpsertRequestCandidateRecord,
    existing: Option<StoredRequestCandidate>,
) -> Result<StoredRequestCandidate, DataLayerError> {
    let created_at_unix_ms = candidate
        .created_at_unix_ms
        .filter(|value| *value > 1000)
        .or_else(|| {
            existing
                .as_ref()
                .map(|value| value.created_at_unix_ms)
                .filter(|value| *value > 1000)
        })
        .or(candidate.started_at_unix_ms)
        .or(candidate.finished_at_unix_ms)
        .unwrap_or_else(current_unix_ms);
    let id = existing
        .as_ref()
        .map(|value| value.id.clone())
        .unwrap_or(candidate.id);
    let extra_data = merge_json_objects(
        existing.as_ref().and_then(|value| value.extra_data.clone()),
        candidate.extra_data,
    );
    StoredRequestCandidate::new(
        id,
        candidate.request_id,
        candidate
            .user_id
            .or_else(|| existing.as_ref().and_then(|value| value.user_id.clone())),
        candidate
            .api_key_id
            .or_else(|| existing.as_ref().and_then(|value| value.api_key_id.clone())),
        candidate
            .username
            .or_else(|| existing.as_ref().and_then(|value| value.username.clone())),
        candidate.api_key_name.or_else(|| {
            existing
                .as_ref()
                .and_then(|value| value.api_key_name.clone())
        }),
        to_i32(candidate.candidate_index)?,
        to_i32(candidate.retry_index)?,
        candidate.provider_id.or_else(|| {
            existing
                .as_ref()
                .and_then(|value| value.provider_id.clone())
        }),
        candidate.endpoint_id.or_else(|| {
            existing
                .as_ref()
                .and_then(|value| value.endpoint_id.clone())
        }),
        candidate
            .key_id
            .or_else(|| existing.as_ref().and_then(|value| value.key_id.clone())),
        candidate.status,
        candidate.skip_reason.or_else(|| {
            existing
                .as_ref()
                .and_then(|value| value.skip_reason.clone())
        }),
        candidate
            .is_cached
            .unwrap_or_else(|| existing.as_ref().is_some_and(|value| value.is_cached)),
        candidate.status_code.map(i32::from).or_else(|| {
            existing
                .as_ref()
                .and_then(|value| value.status_code.map(i32::from))
        }),
        candidate
            .error_type
            .or_else(|| existing.as_ref().and_then(|value| value.error_type.clone())),
        candidate.error_message.or_else(|| {
            existing
                .as_ref()
                .and_then(|value| value.error_message.clone())
        }),
        candidate.latency_ms.map(to_i32_u64).transpose()?.or(
            match existing.as_ref().and_then(|value| value.latency_ms) {
                Some(value) => Some(to_i32_u64(value)?),
                None => None,
            },
        ),
        candidate.concurrent_requests.map(to_i32).transpose()?.or(
            match existing
                .as_ref()
                .and_then(|value| value.concurrent_requests)
            {
                Some(value) => Some(to_i32(value)?),
                None => None,
            },
        ),
        extra_data,
        candidate.required_capabilities.or_else(|| {
            existing
                .as_ref()
                .and_then(|value| value.required_capabilities.clone())
        }),
        u64_to_i64(created_at_unix_ms, "request candidate created_at")?,
        candidate
            .started_at_unix_ms
            .or_else(|| existing.as_ref().and_then(|value| value.started_at_unix_ms))
            .map(|value| u64_to_i64(value, "request candidate started_at"))
            .transpose()?,
        candidate
            .finished_at_unix_ms
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|value| value.finished_at_unix_ms)
            })
            .map(|value| u64_to_i64(value, "request candidate finished_at"))
            .transpose()?,
    )
}

fn aggregate_timeline(
    candidates: Vec<StoredRequestCandidate>,
    since_unix_secs: u64,
    until_unix_secs: u64,
    segments: u32,
) -> Result<Vec<PublicHealthTimelineBucket>, DataLayerError> {
    let endpoint_ids = candidates
        .iter()
        .filter_map(|candidate| candidate.endpoint_id.clone())
        .collect::<BTreeSet<_>>();
    let span_ms = until_unix_secs
        .saturating_sub(since_unix_secs)
        .saturating_mul(1000)
        .max(1);
    let since_ms = since_unix_secs.saturating_mul(1000);
    let mut buckets = BTreeMap::<(String, u32), PublicHealthTimelineBucket>::new();
    for candidate in candidates {
        let Some(endpoint_id) = candidate.endpoint_id.clone() else {
            continue;
        };
        let offset = candidate.created_at_unix_ms.saturating_sub(since_ms);
        let segment_idx = ((offset.saturating_mul(u64::from(segments))) / span_ms)
            .min(u64::from(segments.saturating_sub(1))) as u32;
        let bucket = buckets.entry((endpoint_id.clone(), segment_idx)).or_insert(
            PublicHealthTimelineBucket {
                endpoint_id,
                segment_idx,
                total_count: 0,
                success_count: 0,
                failed_count: 0,
                min_created_at_unix_ms: Some(candidate.created_at_unix_ms),
                max_created_at_unix_ms: Some(candidate.created_at_unix_ms),
            },
        );
        bucket.total_count += 1;
        if candidate.status == RequestCandidateStatus::Success {
            bucket.success_count += 1;
        }
        if candidate.status == RequestCandidateStatus::Failed {
            bucket.failed_count += 1;
        }
        bucket.min_created_at_unix_ms = bucket
            .min_created_at_unix_ms
            .map(|value| value.min(candidate.created_at_unix_ms));
        bucket.max_created_at_unix_ms = bucket
            .max_created_at_unix_ms
            .map(|value| value.max(candidate.created_at_unix_ms));
    }
    for endpoint_id in endpoint_ids {
        for segment_idx in 0..segments {
            buckets.entry((endpoint_id.clone(), segment_idx)).or_insert(
                PublicHealthTimelineBucket {
                    endpoint_id: endpoint_id.clone(),
                    segment_idx,
                    total_count: 0,
                    success_count: 0,
                    failed_count: 0,
                    min_created_at_unix_ms: None,
                    max_created_at_unix_ms: None,
                },
            );
        }
    }
    Ok(buckets.into_values().collect())
}

fn map_candidate_row(row: &MySqlRow) -> Result<StoredRequestCandidate, DataLayerError> {
    StoredRequestCandidate::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("request_id").map_sql_err()?,
        row.try_get("user_id").map_sql_err()?,
        row.try_get("api_key_id").map_sql_err()?,
        row.try_get("username").map_sql_err()?,
        row.try_get("api_key_name").map_sql_err()?,
        row.try_get("candidate_index").map_sql_err()?,
        row.try_get("retry_index").map_sql_err()?,
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("endpoint_id").map_sql_err()?,
        row.try_get("key_id").map_sql_err()?,
        RequestCandidateStatus::from_database(
            row.try_get::<String, _>("status").map_sql_err()?.as_str(),
        )?,
        row.try_get("skip_reason").map_sql_err()?,
        row.try_get("is_cached").map_sql_err()?,
        row.try_get("status_code").map_sql_err()?,
        row.try_get("error_type").map_sql_err()?,
        row.try_get("error_message").map_sql_err()?,
        row.try_get("latency_ms").map_sql_err()?,
        row.try_get("concurrent_requests").map_sql_err()?,
        parse_json(row.try_get("extra_data").ok().flatten())?,
        parse_json(row.try_get("required_capabilities").ok().flatten())?,
        row.try_get("created_at_unix_ms").map_sql_err()?,
        row.try_get("started_at_unix_ms").map_sql_err()?,
        row.try_get("finished_at_unix_ms").map_sql_err()?,
    )
}

fn parse_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "request_candidates JSON field is invalid: {err}"
                ))
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
                    "request_candidates JSON field is unserializable: {err}"
                ))
            })
        })
        .transpose()
}

fn merge_json_objects(
    existing: Option<serde_json::Value>,
    overlay: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match (existing, overlay) {
        (
            Some(serde_json::Value::Object(mut existing_object)),
            Some(serde_json::Value::Object(overlay_object)),
        ) => {
            existing_object.extend(overlay_object);
            Some(serde_json::Value::Object(existing_object))
        }
        (_existing, Some(overlay)) => Some(overlay),
        (existing, None) => existing,
    }
}

fn status_to_database(status: RequestCandidateStatus) -> &'static str {
    match status {
        RequestCandidateStatus::Available => "available",
        RequestCandidateStatus::Unused => "unused",
        RequestCandidateStatus::Pending => "pending",
        RequestCandidateStatus::Streaming => "streaming",
        RequestCandidateStatus::Success => "success",
        RequestCandidateStatus::Failed => "failed",
        RequestCandidateStatus::Cancelled => "cancelled",
        RequestCandidateStatus::Skipped => "skipped",
    }
}

fn current_unix_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn unix_secs_to_ms_i64(value: u64) -> Result<i64, DataLayerError> {
    let value = value.checked_mul(1000).ok_or_else(|| {
        DataLayerError::UnexpectedValue("request candidate timestamp overflow".to_string())
    })?;
    i64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue("request candidate timestamp overflow".to_string())
    })
}

fn limit_i64(value: usize, name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid {name}: {value}")))
}

fn to_i32(value: u32) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("request candidate value out of range: {value}"))
    })
}

fn to_i32_u64(value: u64) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("request candidate value out of range: {value}"))
    })
}

fn u64_to_i64(value: u64, name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| DataLayerError::UnexpectedValue(format!("{name} overflow")))
}

fn optional_u64_to_i64(value: Option<u64>, name: &str) -> Result<Option<i64>, DataLayerError> {
    value.map(|value| u64_to_i64(value, name)).transpose()
}

#[cfg(test)]
mod tests {
    use super::MysqlRequestCandidateRepository;

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlRequestCandidateRepository::new(pool);
    }
}
