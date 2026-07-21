use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use aether_data_contracts::repository::gemini_file_mappings::{
    GeminiFileMappingListQuery, GeminiFileMappingMimeTypeCount, GeminiFileMappingReadRepository,
    GeminiFileMappingStats, GeminiFileMappingWriteRepository, StoredGeminiFileMapping,
    StoredGeminiFileMappingListPage, UpsertGeminiFileMappingRecord,
};
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_ci_contains_any, push_limit_offset, SqlDialect, WhereClause};

use crate::error::SqlResultExt;
use crate::SqlitePool;

#[derive(Debug, Clone)]
pub struct SqliteGeminiFileMappingRepository {
    pool: SqlitePool,
}

impl SqliteGeminiFileMappingRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn reload_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<StoredGeminiFileMapping, DataLayerError> {
        self.find_by_file_name(file_name).await?.ok_or_else(|| {
            DataLayerError::UnexpectedValue("gemini file mapping missing after write".to_string())
        })
    }
}

#[async_trait]
impl GeminiFileMappingReadRepository for SqliteGeminiFileMappingRepository {
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
  created_at AS created_at_unix_ms,
  expires_at AS expires_at_unix_secs
FROM gemini_file_mappings
WHERE file_name = ?
LIMIT 1
"#,
        )
        .bind(file_name)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;

        row.as_ref().map(map_row).transpose()
    }

    async fn list_mappings(
        &self,
        query: &GeminiFileMappingListQuery,
    ) -> Result<StoredGeminiFileMappingListPage, DataLayerError> {
        let total = build_list_count_query(query)
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;
        let rows = build_list_rows_query(query)
            .build()
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        let items = rows.iter().map(map_row).collect::<Result<Vec<_>, _>>()?;
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
  COUNT(*) AS total_mappings,
  SUM(CASE WHEN expires_at > ? THEN 1 ELSE 0 END) AS active_mappings
FROM gemini_file_mappings
"#,
        )
        .bind(now_unix_secs as i64)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        let total_mappings =
            usize::try_from(totals.try_get::<i64, _>("total_mappings").map_sql_err()?)
                .unwrap_or_default();
        let active_mappings = usize::try_from(
            totals
                .try_get::<Option<i64>, _>("active_mappings")
                .map_sql_err()?
                .unwrap_or(0),
        )
        .unwrap_or_default();
        let by_mime_type_rows = sqlx::query(
            r#"
SELECT
  COALESCE(NULLIF(TRIM(mime_type), ''), 'unknown') AS mime_type,
  COUNT(*) AS count
FROM gemini_file_mappings
WHERE expires_at > ?
GROUP BY COALESCE(NULLIF(TRIM(mime_type), ''), 'unknown')
ORDER BY mime_type ASC
"#,
        )
        .bind(now_unix_secs as i64)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let by_mime_type = by_mime_type_rows
            .iter()
            .map(|row| {
                Ok(GeminiFileMappingMimeTypeCount {
                    mime_type: row.try_get("mime_type").map_sql_err()?,
                    count: usize::try_from(row.try_get::<i64, _>("count").map_sql_err()?)
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        Ok(GeminiFileMappingStats {
            total_mappings,
            active_mappings,
            expired_mappings: total_mappings.saturating_sub(active_mappings),
            by_mime_type,
        })
    }
}

#[async_trait]
impl GeminiFileMappingWriteRepository for SqliteGeminiFileMappingRepository {
    async fn upsert(
        &self,
        record: UpsertGeminiFileMappingRecord,
    ) -> Result<StoredGeminiFileMapping, DataLayerError> {
        record.validate()?;
        sqlx::query(
            r#"
INSERT INTO gemini_file_mappings (
  id, file_name, key_id, user_id, display_name, mime_type, source_hash,
  created_at, expires_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(file_name) DO UPDATE SET
  key_id = excluded.key_id,
  user_id = excluded.user_id,
  display_name = excluded.display_name,
  mime_type = excluded.mime_type,
  source_hash = excluded.source_hash,
  expires_at = excluded.expires_at
"#,
        )
        .bind(&record.id)
        .bind(&record.file_name)
        .bind(&record.key_id)
        .bind(&record.user_id)
        .bind(&record.display_name)
        .bind(&record.mime_type)
        .bind(&record.source_hash)
        .bind(current_unix_secs() as i64)
        .bind(i64_from_u64(
            record.expires_at_unix_secs,
            "gemini_file_mappings.expires_at",
        )?)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_by_file_name(&record.file_name).await
    }

    async fn delete_by_file_name(&self, file_name: &str) -> Result<bool, DataLayerError> {
        let rows_affected = sqlx::query("DELETE FROM gemini_file_mappings WHERE file_name = ?")
            .bind(file_name)
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    async fn delete_by_id(
        &self,
        mapping_id: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, DataLayerError> {
        let existing = sqlx::query(
            r#"
SELECT
  id,
  file_name,
  key_id,
  user_id,
  display_name,
  mime_type,
  source_hash,
  created_at AS created_at_unix_ms,
  expires_at AS expires_at_unix_secs
FROM gemini_file_mappings
WHERE id = ?
LIMIT 1
"#,
        )
        .bind(mapping_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        let Some(existing) = existing else {
            return Ok(None);
        };
        sqlx::query("DELETE FROM gemini_file_mappings WHERE id = ?")
            .bind(mapping_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        Ok(Some(map_row(&existing)?))
    }

    async fn delete_expired_before(&self, now_unix_secs: u64) -> Result<usize, DataLayerError> {
        let rows_affected = sqlx::query("DELETE FROM gemini_file_mappings WHERE expires_at <= ?")
            .bind(now_unix_secs as i64)
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(usize::try_from(rows_affected).unwrap_or_default())
    }
}

fn build_list_count_query(query: &GeminiFileMappingListQuery) -> QueryBuilder<'_, Sqlite> {
    let mut builder =
        QueryBuilder::<Sqlite>::new("SELECT COUNT(*) AS total FROM gemini_file_mappings");
    let mut where_clause = WhereClause::new();
    apply_list_filters(&mut builder, &mut where_clause, query);
    builder
}

fn build_list_rows_query(query: &GeminiFileMappingListQuery) -> QueryBuilder<'_, Sqlite> {
    let mut builder = QueryBuilder::<Sqlite>::new(
        r#"
SELECT
  id,
  file_name,
  key_id,
  user_id,
  display_name,
  mime_type,
  source_hash,
  created_at AS created_at_unix_ms,
  expires_at AS expires_at_unix_secs
FROM gemini_file_mappings
"#,
    );
    let mut where_clause = WhereClause::new();
    apply_list_filters(&mut builder, &mut where_clause, query);
    builder.push(" ORDER BY created_at DESC, file_name ASC");
    push_limit_offset(
        &mut builder,
        i64::try_from(query.limit).unwrap_or(i64::MAX),
        i64::try_from(query.offset).unwrap_or(i64::MAX),
    );
    builder
}

fn apply_list_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    where_clause: &mut WhereClause,
    query: &GeminiFileMappingListQuery,
) {
    if !query.include_expired {
        where_clause.push_next(builder);
        builder.push("expires_at > ");
        builder.push_bind(query.now_unix_secs as i64);
    }
    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_ci_contains_any(
            builder,
            where_clause,
            SqlDialect::Sqlite,
            &["file_name", "COALESCE(display_name, '')"],
            search,
        );
    }
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn i64_from_u64(value: u64, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field_name} exceeds i64: {value}")))
}

fn map_row(row: &SqliteRow) -> Result<StoredGeminiFileMapping, DataLayerError> {
    Ok(StoredGeminiFileMapping {
        id: row.try_get("id").map_sql_err()?,
        file_name: row.try_get("file_name").map_sql_err()?,
        key_id: row.try_get("key_id").map_sql_err()?,
        user_id: row.try_get("user_id").ok().flatten(),
        display_name: row.try_get("display_name").ok().flatten(),
        mime_type: row.try_get("mime_type").ok().flatten(),
        source_hash: row.try_get("source_hash").ok().flatten(),
        created_at_unix_ms: u64::try_from(
            row.try_get::<i64, _>("created_at_unix_ms").map_sql_err()?,
        )
        .map_err(|_| {
            DataLayerError::UnexpectedValue(
                "gemini_file_mappings.created_at is invalid".to_string(),
            )
        })?,
        expires_at_unix_secs: u64::try_from(
            row.try_get::<i64, _>("expires_at_unix_secs")
                .map_sql_err()?,
        )
        .map_err(|_| {
            DataLayerError::UnexpectedValue(
                "gemini_file_mappings.expires_at is invalid".to_string(),
            )
        })?,
    })
}

#[cfg(test)]
mod tests {
    use super::SqliteGeminiFileMappingRepository;
    use crate::run_migrations;
    use aether_data_contracts::repository::gemini_file_mappings::{
        GeminiFileMappingListQuery, GeminiFileMappingReadRepository,
        GeminiFileMappingWriteRepository, UpsertGeminiFileMappingRecord,
    };

    #[tokio::test]
    async fn sqlite_repository_round_trips_gemini_file_mappings() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let repository = SqliteGeminiFileMappingRepository::new(pool);
        let created = repository
            .upsert(UpsertGeminiFileMappingRecord {
                id: "mapping-1".to_string(),
                file_name: "files/example.png".to_string(),
                key_id: "key-1".to_string(),
                user_id: Some("user-1".to_string()),
                display_name: Some("Example".to_string()),
                mime_type: Some("image/png".to_string()),
                source_hash: Some("hash-1".to_string()),
                expires_at_unix_secs: 300,
            })
            .await
            .expect("mapping should upsert");
        assert_eq!(created.id, "mapping-1");
        assert_eq!(created.mime_type, Some("image/png".to_string()));

        let updated = repository
            .upsert(UpsertGeminiFileMappingRecord {
                id: "mapping-replacement".to_string(),
                file_name: "files/example.png".to_string(),
                key_id: "key-2".to_string(),
                user_id: Some("user-2".to_string()),
                display_name: Some("Updated".to_string()),
                mime_type: Some("image/jpeg".to_string()),
                source_hash: Some("hash-2".to_string()),
                expires_at_unix_secs: 500,
            })
            .await
            .expect("mapping should update");
        assert_eq!(updated.id, "mapping-1");
        assert_eq!(updated.key_id, "key-2");

        let page = repository
            .list_mappings(&GeminiFileMappingListQuery {
                include_expired: false,
                search: Some("updated".to_string()),
                offset: 0,
                limit: 10,
                now_unix_secs: 400,
            })
            .await
            .expect("mappings should list");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].file_name, "files/example.png");

        let stats = repository
            .summarize_mappings(400)
            .await
            .expect("stats should load");
        assert_eq!(stats.total_mappings, 1);
        assert_eq!(stats.active_mappings, 1);
        assert_eq!(stats.by_mime_type[0].mime_type, "image/jpeg");

        assert_eq!(
            repository
                .delete_expired_before(600)
                .await
                .expect("expired mappings should delete"),
            1
        );
        assert!(repository
            .find_by_file_name("files/example.png")
            .await
            .expect("find should run")
            .is_none());
    }
}
