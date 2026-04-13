use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use super::types::{
    GeminiFileMappingListQuery, GeminiFileMappingMimeTypeCount, GeminiFileMappingReadRepository,
    GeminiFileMappingStats, GeminiFileMappingWriteRepository, StoredGeminiFileMapping,
    StoredGeminiFileMappingListPage, UpsertGeminiFileMappingRecord,
};
use crate::{error::SqlxResultExt, DataLayerError};

#[derive(Debug, Clone)]
pub struct SqlxGeminiFileMappingRepository {
    pool: PgPool,
}

impl SqlxGeminiFileMappingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn map_row(row: &PgRow) -> Result<StoredGeminiFileMapping, DataLayerError> {
        Ok(StoredGeminiFileMapping {
            id: row.try_get("id").map_postgres_err()?,
            file_name: row.try_get("file_name").map_postgres_err()?,
            key_id: row.try_get("key_id").map_postgres_err()?,
            user_id: row.try_get("user_id").ok().flatten(),
            display_name: row.try_get("display_name").ok().flatten(),
            mime_type: row.try_get("mime_type").ok().flatten(),
            source_hash: row.try_get("source_hash").ok().flatten(),
            created_at_unix_ms: u64::try_from(
                row.try_get::<i64, _>("created_at_unix_ms")
                    .map_postgres_err()?,
            )
            .map_err(|_| {
                DataLayerError::UnexpectedValue(
                    "gemini_file_mappings.created_at is invalid".to_string(),
                )
            })?,
            expires_at_unix_secs: u64::try_from(
                row.try_get::<i64, _>("expires_at_unix_secs")
                    .map_postgres_err()?,
            )
            .map_err(|_| {
                DataLayerError::UnexpectedValue(
                    "gemini_file_mappings.expires_at is invalid".to_string(),
                )
            })?,
        })
    }
}

#[async_trait]
impl GeminiFileMappingReadRepository for SqlxGeminiFileMappingRepository {
    async fn find_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  id,
  file_name,
  key_id,
  user_id,
  display_name,
  mime_type,
  source_hash,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs
FROM gemini_file_mappings
WHERE file_name = $1
"#,
        )
        .bind(file_name)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        match row {
            Some(row) => Ok(Some(Self::map_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_mappings(
        &self,
        query: &GeminiFileMappingListQuery,
    ) -> Result<StoredGeminiFileMappingListPage, DataLayerError> {
        let total = build_list_count_query(query)
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let mut builder = build_list_rows_query(query);
        let built_query = builder.build();
        let mut rows = built_query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(Self::map_row(&row)?);
        }
        Ok(StoredGeminiFileMappingListPage {
            items,
            total: usize::try_from(total).unwrap_or_default(),
        })
    }

    async fn summarize_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<GeminiFileMappingStats, DataLayerError> {
        let totals = sqlx::query(
            r#"
SELECT
  COUNT(*)::bigint AS total_mappings,
  COUNT(*) FILTER (WHERE expires_at > TO_TIMESTAMP($1::double precision))::bigint AS active_mappings
FROM gemini_file_mappings
"#,
        )
        .bind(now_unix_secs as f64)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        let total_mappings = usize::try_from(
            totals
                .try_get::<i64, _>("total_mappings")
                .map_postgres_err()?,
        )
        .unwrap_or_default();
        let active_mappings = usize::try_from(
            totals
                .try_get::<i64, _>("active_mappings")
                .map_postgres_err()?,
        )
        .unwrap_or_default();
        let mut by_mime_type_rows = sqlx::query(
            r#"
SELECT
  COALESCE(NULLIF(TRIM(mime_type), ''), 'unknown') AS mime_type,
  COUNT(*)::bigint AS count
FROM gemini_file_mappings
WHERE expires_at > TO_TIMESTAMP($1::double precision)
GROUP BY COALESCE(NULLIF(TRIM(mime_type), ''), 'unknown')
ORDER BY mime_type ASC
"#,
        )
        .bind(now_unix_secs as f64)
        .fetch(&self.pool);
        let mut by_mime_type = Vec::new();
        while let Some(row) = by_mime_type_rows.try_next().await.map_postgres_err()? {
            by_mime_type.push(GeminiFileMappingMimeTypeCount {
                mime_type: row.try_get("mime_type").map_postgres_err()?,
                count: usize::try_from(row.try_get::<i64, _>("count").map_postgres_err()?)
                    .unwrap_or_default(),
            });
        }
        Ok(GeminiFileMappingStats {
            total_mappings,
            active_mappings,
            expired_mappings: total_mappings.saturating_sub(active_mappings),
            by_mime_type,
        })
    }
}

#[async_trait]
impl GeminiFileMappingWriteRepository for SqlxGeminiFileMappingRepository {
    async fn upsert(
        &self,
        record: UpsertGeminiFileMappingRecord,
    ) -> Result<StoredGeminiFileMapping, DataLayerError> {
        record.validate()?;
        let row = sqlx::query(
            r#"
INSERT INTO gemini_file_mappings (
  id,
  file_name,
  key_id,
  user_id,
  display_name,
  mime_type,
  source_hash,
  created_at,
  expires_at
)
VALUES ($1,$2,$3,$4,$5,$6,$7,NOW(),TO_TIMESTAMP($8::double precision))
ON CONFLICT (file_name)
DO UPDATE
SET
  key_id = EXCLUDED.key_id,
  user_id = EXCLUDED.user_id,
  display_name = EXCLUDED.display_name,
  mime_type = EXCLUDED.mime_type,
  source_hash = EXCLUDED.source_hash,
  expires_at = EXCLUDED.expires_at
RETURNING
  id,
  file_name,
  key_id,
  user_id,
  display_name,
  mime_type,
  source_hash,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs
"#,
        )
        .bind(record.id.clone())
        .bind(record.file_name.clone())
        .bind(record.key_id.clone())
        .bind(record.user_id.clone())
        .bind(record.display_name.clone())
        .bind(record.mime_type.clone())
        .bind(record.source_hash.clone())
        .bind(record.expires_at_unix_secs as f64)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;

        Self::map_row(&row)
    }

    async fn delete_by_file_name(&self, file_name: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
DELETE FROM gemini_file_mappings
WHERE file_name = $1
#"#,
        )
        .bind(file_name)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;

        Ok(result.rows_affected() > 0)
    }

    async fn delete_by_id(
        &self,
        mapping_id: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, DataLayerError> {
        let row = sqlx::query(
            r#"
DELETE FROM gemini_file_mappings
WHERE id = $1
RETURNING
  id,
  file_name,
  key_id,
  user_id,
  display_name,
  mime_type,
  source_hash,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs
"#,
        )
        .bind(mapping_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        match row {
            Some(row) => Ok(Some(Self::map_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn delete_expired_before(&self, now_unix_secs: u64) -> Result<usize, DataLayerError> {
        let result = sqlx::query(
            r#"
DELETE FROM gemini_file_mappings
WHERE expires_at <= TO_TIMESTAMP($1::double precision)
"#,
        )
        .bind(now_unix_secs as f64)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;

        Ok(usize::try_from(result.rows_affected()).unwrap_or_default())
    }
}

fn build_list_count_query(query: &GeminiFileMappingListQuery) -> QueryBuilder<'_, Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(
        "SELECT COUNT(*)::bigint AS total FROM gemini_file_mappings WHERE 1=1",
    );
    apply_list_filters(&mut builder, query);
    builder
}

fn build_list_rows_query(query: &GeminiFileMappingListQuery) -> QueryBuilder<'_, Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
SELECT
  id,
  file_name,
  key_id,
  user_id,
  display_name,
  mime_type,
  source_hash,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs
FROM gemini_file_mappings
WHERE 1=1
"#,
    );
    apply_list_filters(&mut builder, query);
    builder.push(" ORDER BY created_at DESC, file_name ASC LIMIT ");
    builder.push_bind(i64::try_from(query.limit).unwrap_or(i64::MAX));
    builder.push(" OFFSET ");
    builder.push_bind(i64::try_from(query.offset).unwrap_or(i64::MAX));
    builder
}

fn apply_list_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    query: &GeminiFileMappingListQuery,
) {
    if !query.include_expired {
        builder.push(" AND expires_at > TO_TIMESTAMP(");
        builder.push_bind(query.now_unix_secs as f64);
        builder.push("::double precision)");
    }
    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let pattern = format!("%{search}%");
        builder.push(" AND (file_name ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR COALESCE(display_name, '') ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    }
}
