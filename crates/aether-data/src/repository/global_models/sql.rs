use async_trait::async_trait;
use futures_util::{stream::TryStream, TryStreamExt};
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use super::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, CreateAdminGlobalModelRecord,
    GlobalModelReadRepository, GlobalModelWriteRepository, PublicCatalogModelListQuery,
    PublicCatalogModelSearchQuery, PublicGlobalModelQuery, StoredAdminGlobalModel,
    StoredAdminGlobalModelPage, StoredAdminProviderModel, StoredProviderActiveGlobalModel,
    StoredProviderModelStats, StoredPublicCatalogModel, StoredPublicGlobalModel,
    StoredPublicGlobalModelPage, UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use crate::{error::SqlxResultExt, DataLayerError};

const LIST_PUBLIC_GLOBAL_MODELS_PREFIX: &str = r#"
SELECT
  id,
  name,
  display_name,
  is_active,
  CAST(default_price_per_request AS DOUBLE PRECISION) AS default_price_per_request,
  default_tiered_pricing,
  supported_capabilities,
  config
FROM global_models
"#;

const COUNT_PUBLIC_GLOBAL_MODELS_PREFIX: &str = r#"
SELECT COUNT(id) AS total
FROM global_models
"#;

const LIST_PUBLIC_CATALOG_MODELS_PREFIX: &str = r#"
SELECT
  m.id,
  m.provider_id,
  p.name AS provider_name,
  m.provider_model_name,
  COALESCE(gm.name, m.provider_model_name) AS name,
  COALESCE(NULLIF(gm.display_name, ''), m.provider_model_name) AS display_name,
  CASE
    WHEN gm.config IS NULL THEN NULL
    ELSE gm.config->>'description'
  END AS description,
  CASE
    WHEN gm.config IS NULL THEN NULL
    ELSE gm.config->>'icon_url'
  END AS icon_url,
  COALESCE(
    CAST((COALESCE(m.tiered_pricing, gm.default_tiered_pricing)->'tiers'->0->>'input_price_per_1m') AS DOUBLE PRECISION),
    0.0
  ) AS input_price_per_1m,
  COALESCE(
    CAST((COALESCE(m.tiered_pricing, gm.default_tiered_pricing)->'tiers'->0->>'output_price_per_1m') AS DOUBLE PRECISION),
    0.0
  ) AS output_price_per_1m,
  CAST((COALESCE(m.tiered_pricing, gm.default_tiered_pricing)->'tiers'->0->>'cache_creation_price_per_1m') AS DOUBLE PRECISION) AS cache_creation_price_per_1m,
  CAST((COALESCE(m.tiered_pricing, gm.default_tiered_pricing)->'tiers'->0->>'cache_read_price_per_1m') AS DOUBLE PRECISION) AS cache_read_price_per_1m,
  COALESCE(m.supports_vision, CAST(gm.config->>'vision' AS BOOLEAN), FALSE) AS supports_vision,
  COALESCE(m.supports_function_calling, CAST(gm.config->>'function_calling' AS BOOLEAN), FALSE) AS supports_function_calling,
  COALESCE(m.supports_streaming, CAST(gm.config->>'streaming' AS BOOLEAN), TRUE) AS supports_streaming,
  m.is_active
FROM models m
JOIN providers p ON p.id = m.provider_id
LEFT JOIN global_models gm ON gm.id = m.global_model_id
"#;

const LIST_PROVIDER_MODEL_STATS_PREFIX: &str = r#"
SELECT
  provider_id,
  COUNT(id) AS total_models,
  COALESCE(SUM(CASE WHEN is_active = TRUE THEN 1 ELSE 0 END), 0) AS active_models
FROM models
WHERE provider_id IN (
"#;

const LIST_ADMIN_PROVIDER_MODELS_PREFIX: &str = r#"
SELECT
  m.id,
  m.provider_id,
  m.global_model_id,
  m.provider_model_name,
  m.provider_model_mappings,
  CAST(m.price_per_request AS DOUBLE PRECISION) AS price_per_request,
  m.tiered_pricing,
  m.supports_vision,
  m.supports_function_calling,
  m.supports_streaming,
  m.supports_extended_thinking,
  m.supports_image_generation,
  m.is_active,
  COALESCE(m.is_available, TRUE) AS is_available,
  m.config,
  EXTRACT(EPOCH FROM m.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM m.updated_at)::bigint AS updated_at_unix_secs,
  gm.name AS global_model_name,
  gm.display_name AS global_model_display_name,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS global_model_default_price_per_request,
  gm.default_tiered_pricing AS global_model_default_tiered_pricing,
  gm.config AS global_model_config
FROM models m
LEFT JOIN global_models gm ON gm.id = m.global_model_id
"#;

const LIST_ADMIN_GLOBAL_MODELS_PREFIX: &str = r#"
SELECT
  gm.id,
  gm.name,
  COALESCE(NULLIF(gm.display_name, ''), gm.name) AS display_name,
  gm.is_active,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS default_price_per_request,
  gm.default_tiered_pricing,
  gm.supported_capabilities,
  gm.config,
  COALESCE(gm_stats.provider_count, 0) AS provider_count,
  COALESCE(gm_stats.active_provider_count, 0) AS active_provider_count,
  COALESCE(gm.usage_count, 0)::bigint AS usage_count,
  EXTRACT(EPOCH FROM gm.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM gm.updated_at)::bigint AS updated_at_unix_secs
FROM global_models gm
LEFT JOIN (
  SELECT
    m.global_model_id,
    COUNT(DISTINCT m.provider_id)::bigint AS provider_count,
    COUNT(
      DISTINCT CASE
        WHEN m.is_active = TRUE AND COALESCE(m.is_available, TRUE) = TRUE AND p.is_active = TRUE THEN m.provider_id
        ELSE NULL
      END
    )::bigint AS active_provider_count
  FROM models m
  JOIN providers p ON p.id = m.provider_id
  GROUP BY m.global_model_id
) gm_stats ON gm_stats.global_model_id = gm.id
"#;

const COUNT_ADMIN_GLOBAL_MODELS_PREFIX: &str = r#"
SELECT COUNT(id) AS total
FROM global_models gm
"#;

const LIST_ACTIVE_GLOBAL_MODEL_IDS_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT DISTINCT
  provider_id,
  global_model_id
FROM models
WHERE provider_id IN (
"#;

#[derive(Debug, Clone)]
pub struct SqlxGlobalModelReadRepository {
    pool: PgPool,
}

impl SqlxGlobalModelReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, DataLayerError> {
        let mut count_builder = QueryBuilder::<Postgres>::new(COUNT_PUBLIC_GLOBAL_MODELS_PREFIX);
        apply_public_model_filters(&mut count_builder, query);
        let count_row = count_builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let total = count_row
            .try_get::<i64, _>("total")
            .map(|value| value.max(0) as usize)
            .map_postgres_err()?;

        let mut list_builder = QueryBuilder::<Postgres>::new(LIST_PUBLIC_GLOBAL_MODELS_PREFIX);
        apply_public_model_filters(&mut list_builder, query);
        list_builder
            .push(" ORDER BY name ASC OFFSET ")
            .push_bind(query.offset as i64)
            .push(" LIMIT ")
            .push_bind(query.limit as i64);
        let query = list_builder.build();
        let items = collect_query_rows(query.fetch(&self.pool), map_row).await?;

        Ok(StoredPublicGlobalModelPage { items, total })
    }

    pub async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = build_provider_id_list_query(
            LIST_PROVIDER_MODEL_STATS_PREFIX,
            provider_ids,
            ")\nGROUP BY provider_id\nORDER BY provider_id ASC",
        );
        let query = builder.build();
        collect_query_rows(query.fetch(&self.pool), map_provider_model_stats_row).await
    }

    pub async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = build_provider_id_list_query(
            LIST_ACTIVE_GLOBAL_MODEL_IDS_BY_PROVIDER_IDS_PREFIX,
            provider_ids,
            ")\nAND is_active = TRUE\nAND global_model_id IS NOT NULL\nORDER BY provider_id ASC, global_model_id ASC",
        );
        let query = builder.build();
        collect_query_rows(
            query.fetch(&self.pool),
            map_provider_active_global_model_row,
        )
        .await
    }

    pub async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_ADMIN_PROVIDER_MODELS_PREFIX);
        builder
            .push(" WHERE m.provider_id = ")
            .push_bind(query.provider_id.trim().to_string());
        if let Some(is_active) = query.is_active {
            builder.push(" AND m.is_active = ").push_bind(is_active);
        }
        builder
            .push(" ORDER BY m.created_at DESC, m.id ASC OFFSET ")
            .push_bind(query.offset as i64)
            .push(" LIMIT ")
            .push_bind(query.limit as i64);
        let query = builder.build();
        collect_query_rows(query.fetch(&self.pool), map_admin_provider_model_row).await
    }

    pub async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, DataLayerError> {
        let mut count_builder = QueryBuilder::<Postgres>::new(COUNT_ADMIN_GLOBAL_MODELS_PREFIX);
        apply_admin_global_model_filters(&mut count_builder, query);
        let count_row = count_builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let total = count_row
            .try_get::<i64, _>("total")
            .map(|value| value.max(0) as usize)
            .map_postgres_err()?;

        let mut list_builder = QueryBuilder::<Postgres>::new(LIST_ADMIN_GLOBAL_MODELS_PREFIX);
        apply_admin_global_model_filters(&mut list_builder, query);
        list_builder
            .push(" ORDER BY name ASC OFFSET ")
            .push_bind(query.offset as i64)
            .push(" LIMIT ")
            .push_bind(query.limit as i64);
        let query = list_builder.build();
        let items = collect_query_rows(query.fetch(&self.pool), map_admin_global_model_row).await?;
        Ok(StoredAdminGlobalModelPage { items, total })
    }

    pub async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  m.id,
  m.provider_id,
  m.global_model_id,
  m.provider_model_name,
  m.provider_model_mappings,
  CAST(m.price_per_request AS DOUBLE PRECISION) AS price_per_request,
  m.tiered_pricing,
  m.supports_vision,
  m.supports_function_calling,
  m.supports_streaming,
  m.supports_extended_thinking,
  m.supports_image_generation,
  m.is_active,
  COALESCE(m.is_available, TRUE) AS is_available,
  m.config,
  EXTRACT(EPOCH FROM m.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM m.updated_at)::bigint AS updated_at_unix_secs,
  gm.name AS global_model_name,
  gm.display_name AS global_model_display_name,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS global_model_default_price_per_request,
  gm.default_tiered_pricing AS global_model_default_tiered_pricing,
  gm.config AS global_model_config
FROM models m
LEFT JOIN global_models gm ON gm.id = m.global_model_id
WHERE m.provider_id = $1
  AND m.id = $2
LIMIT 1
            "#,
        )
        .bind(provider_id)
        .bind(model_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        row.as_ref().map(map_admin_provider_model_row).transpose()
    }

    pub async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        collect_query_rows(
            sqlx::query(
                r#"
SELECT
  m.id,
  m.provider_id,
  m.global_model_id,
  m.provider_model_name,
  m.provider_model_mappings,
  CAST(m.price_per_request AS DOUBLE PRECISION) AS price_per_request,
  m.tiered_pricing,
  m.supports_vision,
  m.supports_function_calling,
  m.supports_streaming,
  m.supports_extended_thinking,
  m.supports_image_generation,
  m.is_active,
  COALESCE(m.is_available, TRUE) AS is_available,
  m.config,
  EXTRACT(EPOCH FROM m.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM m.updated_at)::bigint AS updated_at_unix_secs,
  gm.name AS global_model_name,
  gm.display_name AS global_model_display_name,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS global_model_default_price_per_request,
  gm.default_tiered_pricing AS global_model_default_tiered_pricing,
  gm.config AS global_model_config
FROM models m
JOIN global_models gm ON gm.id = m.global_model_id
WHERE m.provider_id = $1
  AND m.is_active = TRUE
  AND gm.is_active = TRUE
ORDER BY gm.name ASC, m.created_at DESC, m.id ASC
            "#,
            )
            .bind(provider_id)
            .fetch(&self.pool),
            map_admin_provider_model_row,
        )
        .await
    }

    pub async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  gm.id,
  gm.name,
  COALESCE(NULLIF(gm.display_name, ''), gm.name) AS display_name,
  gm.is_active,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS default_price_per_request,
  gm.default_tiered_pricing,
  gm.supported_capabilities,
  gm.config,
  COALESCE(gm_stats.provider_count, 0) AS provider_count,
  COALESCE(gm_stats.active_provider_count, 0) AS active_provider_count,
  COALESCE(gm.usage_count, 0)::bigint AS usage_count,
  EXTRACT(EPOCH FROM gm.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM gm.updated_at)::bigint AS updated_at_unix_secs
FROM global_models gm
LEFT JOIN (
  SELECT
    m.global_model_id,
    COUNT(DISTINCT m.provider_id)::bigint AS provider_count,
    COUNT(
      DISTINCT CASE
        WHEN m.is_active = TRUE AND COALESCE(m.is_available, TRUE) = TRUE AND p.is_active = TRUE THEN m.provider_id
        ELSE NULL
      END
    )::bigint AS active_provider_count
  FROM models m
  JOIN providers p ON p.id = m.provider_id
  GROUP BY m.global_model_id
) gm_stats ON gm_stats.global_model_id = gm.id
WHERE gm.id = $1
LIMIT 1
            "#,
        )
        .bind(global_model_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        row.as_ref().map(map_admin_global_model_row).transpose()
    }

    pub async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  gm.id,
  gm.name,
  COALESCE(NULLIF(gm.display_name, ''), gm.name) AS display_name,
  gm.is_active,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS default_price_per_request,
  gm.default_tiered_pricing,
  gm.supported_capabilities,
  gm.config,
  COALESCE(gm_stats.provider_count, 0) AS provider_count,
  COALESCE(gm_stats.active_provider_count, 0) AS active_provider_count,
  COALESCE(gm.usage_count, 0)::bigint AS usage_count,
  EXTRACT(EPOCH FROM gm.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM gm.updated_at)::bigint AS updated_at_unix_secs
FROM global_models gm
LEFT JOIN (
  SELECT
    m.global_model_id,
    COUNT(DISTINCT m.provider_id)::bigint AS provider_count,
    COUNT(
      DISTINCT CASE
        WHEN m.is_active = TRUE AND COALESCE(m.is_available, TRUE) = TRUE AND p.is_active = TRUE THEN m.provider_id
        ELSE NULL
      END
    )::bigint AS active_provider_count
  FROM models m
  JOIN providers p ON p.id = m.provider_id
  GROUP BY m.global_model_id
) gm_stats ON gm_stats.global_model_id = gm.id
WHERE gm.name = $1
LIMIT 1
            "#,
        )
        .bind(model_name)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        row.as_ref().map(map_admin_global_model_row).transpose()
    }

    pub async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        collect_query_rows(
            sqlx::query(
                r#"
SELECT
  m.id,
  m.provider_id,
  m.global_model_id,
  m.provider_model_name,
  m.provider_model_mappings,
  CAST(m.price_per_request AS DOUBLE PRECISION) AS price_per_request,
  m.tiered_pricing,
  m.supports_vision,
  m.supports_function_calling,
  m.supports_streaming,
  m.supports_extended_thinking,
  m.supports_image_generation,
  m.is_active,
  COALESCE(m.is_available, TRUE) AS is_available,
  m.config,
  EXTRACT(EPOCH FROM m.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM m.updated_at)::bigint AS updated_at_unix_secs,
  gm.name AS global_model_name,
  gm.display_name AS global_model_display_name,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS global_model_default_price_per_request,
  gm.default_tiered_pricing AS global_model_default_tiered_pricing,
  gm.config AS global_model_config
FROM models m
LEFT JOIN global_models gm ON gm.id = m.global_model_id
WHERE m.global_model_id = $1
ORDER BY m.created_at DESC, m.id ASC
            "#,
            )
            .bind(global_model_id)
            .fetch(&self.pool),
            map_admin_provider_model_row,
        )
        .await
    }

    pub async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let inserted = sqlx::query(
            r#"
INSERT INTO models (
  id,
  provider_id,
  global_model_id,
  provider_model_name,
  provider_model_mappings,
  price_per_request,
  tiered_pricing,
  supports_vision,
  supports_function_calling,
  supports_streaming,
  supports_extended_thinking,
  supports_image_generation,
  is_active,
  is_available,
  config,
  created_at,
  updated_at
)
VALUES (
  $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, NOW(), NOW()
)
RETURNING id
            "#,
        )
        .bind(&record.id)
        .bind(&record.provider_id)
        .bind(&record.global_model_id)
        .bind(&record.provider_model_name)
        .bind(record.provider_model_mappings.clone())
        .bind(record.price_per_request)
        .bind(record.tiered_pricing.clone())
        .bind(record.supports_vision)
        .bind(record.supports_function_calling)
        .bind(record.supports_streaming)
        .bind(record.supports_extended_thinking)
        .bind(record.supports_image_generation)
        .bind(record.is_active)
        .bind(record.is_available)
        .bind(record.config.clone())
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        if inserted.is_none() {
            return Ok(None);
        }

        self.get_admin_provider_model(&record.provider_id, &record.id)
            .await
    }

    pub async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let updated = sqlx::query(
            r#"
UPDATE models
SET
  global_model_id = $3,
  provider_model_name = $4,
  provider_model_mappings = $5,
  price_per_request = $6,
  tiered_pricing = $7,
  supports_vision = $8,
  supports_function_calling = $9,
  supports_streaming = $10,
  supports_extended_thinking = $11,
  supports_image_generation = $12,
  is_active = $13,
  is_available = $14,
  config = $15,
  updated_at = NOW()
WHERE id = $1
  AND provider_id = $2
RETURNING id
            "#,
        )
        .bind(&record.id)
        .bind(&record.provider_id)
        .bind(&record.global_model_id)
        .bind(&record.provider_model_name)
        .bind(record.provider_model_mappings.clone())
        .bind(record.price_per_request)
        .bind(record.tiered_pricing.clone())
        .bind(record.supports_vision)
        .bind(record.supports_function_calling)
        .bind(record.supports_streaming)
        .bind(record.supports_extended_thinking)
        .bind(record.supports_image_generation)
        .bind(record.is_active)
        .bind(record.is_available)
        .bind(record.config.clone())
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        if updated.is_none() {
            return Ok(None);
        }

        self.get_admin_provider_model(&record.provider_id, &record.id)
            .await
    }

    pub async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let deleted = sqlx::query(
            r#"
DELETE FROM models
WHERE provider_id = $1
  AND id = $2
RETURNING id
            "#,
        )
        .bind(provider_id)
        .bind(model_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        Ok(deleted.is_some())
    }

    pub async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let inserted = sqlx::query(
            r#"
INSERT INTO global_models (
  id,
  name,
  display_name,
  is_active,
  default_price_per_request,
  default_tiered_pricing,
  supported_capabilities,
  usage_count,
  config,
  created_at,
  updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW(), NOW())
RETURNING id
            "#,
        )
        .bind(&record.id)
        .bind(&record.name)
        .bind(&record.display_name)
        .bind(record.is_active)
        .bind(record.default_price_per_request)
        .bind(record.default_tiered_pricing.clone())
        .bind(record.supported_capabilities.clone())
        .bind(0_i32)
        .bind(record.config.clone())
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        if inserted.is_none() {
            return Ok(None);
        }

        self.get_admin_global_model_by_id(&record.id).await
    }

    pub async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let updated = sqlx::query(
            r#"
UPDATE global_models
SET
  display_name = $2,
  is_active = $3,
  default_price_per_request = $4,
  default_tiered_pricing = $5,
  supported_capabilities = $6,
  config = $7,
  updated_at = NOW()
WHERE id = $1
RETURNING id
            "#,
        )
        .bind(&record.id)
        .bind(&record.display_name)
        .bind(record.is_active)
        .bind(record.default_price_per_request)
        .bind(record.default_tiered_pricing.clone())
        .bind(record.supported_capabilities.clone())
        .bind(record.config.clone())
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        if updated.is_none() {
            return Ok(None);
        }

        self.get_admin_global_model_by_id(&record.id).await
    }

    pub async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let deleted = sqlx::query(
            r#"
DELETE FROM global_models
WHERE id = $1
RETURNING id
            "#,
        )
        .bind(global_model_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        Ok(deleted.is_some())
    }
}

#[async_trait]
impl GlobalModelReadRepository for SqlxGlobalModelReadRepository {
    async fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, DataLayerError> {
        Self::list_public_models(self, query).await
    }

    async fn get_public_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredPublicGlobalModel>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  id,
  name,
  display_name,
  is_active,
  CAST(default_price_per_request AS DOUBLE PRECISION) AS default_price_per_request,
  default_tiered_pricing,
  supported_capabilities,
  config
FROM global_models
WHERE name = $1 AND is_active = TRUE
LIMIT 1
            "#,
        )
        .bind(model_name)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        row.as_ref().map(map_row).transpose()
    }

    async fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_PUBLIC_CATALOG_MODELS_PREFIX);
        apply_public_catalog_model_filters(&mut builder, query.provider_id.as_deref(), None);
        builder
            .push(" ORDER BY p.provider_priority ASC, p.name ASC, COALESCE(gm.name, m.provider_model_name) ASC, m.id ASC OFFSET ")
            .push_bind(query.offset as i64)
            .push(" LIMIT ")
            .push_bind(query.limit as i64);
        let query = builder.build();
        collect_query_rows(query.fetch(&self.pool), map_public_catalog_model_row).await
    }

    async fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_PUBLIC_CATALOG_MODELS_PREFIX);
        apply_public_catalog_model_filters(
            &mut builder,
            query.provider_id.as_deref(),
            Some(query.search.as_str()),
        );
        builder
            .push(" ORDER BY p.provider_priority ASC, p.name ASC, COALESCE(gm.name, m.provider_model_name) ASC, m.id ASC LIMIT ")
            .push_bind(query.limit as i64);
        let query = builder.build();
        collect_query_rows(query.fetch(&self.pool), map_public_catalog_model_row).await
    }

    async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, DataLayerError> {
        Self::list_admin_global_models(self, query).await
    }

    async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Self::list_admin_provider_models(self, query).await
    }

    async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Self::get_admin_provider_model(self, provider_id, model_id).await
    }

    async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Self::list_admin_provider_available_source_models(self, provider_id).await
    }

    async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Self::get_admin_global_model_by_id(self, global_model_id).await
    }

    async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Self::get_admin_global_model_by_name(self, model_name).await
    }

    async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Self::list_admin_provider_models_by_global_model_id(self, global_model_id).await
    }

    async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, DataLayerError> {
        Self::list_provider_model_stats(self, provider_ids).await
    }

    async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, DataLayerError> {
        Self::list_active_global_model_ids_by_provider_ids(self, provider_ids).await
    }
}

#[async_trait]
impl GlobalModelWriteRepository for SqlxGlobalModelReadRepository {
    async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Self::create_admin_provider_model(self, record).await
    }

    async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Self::update_admin_provider_model(self, record).await
    }

    async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, DataLayerError> {
        Self::delete_admin_provider_model(self, provider_id, model_id).await
    }

    async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Self::create_admin_global_model(self, record).await
    }

    async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Self::update_admin_global_model(self, record).await
    }

    async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, DataLayerError> {
        Self::delete_admin_global_model(self, global_model_id).await
    }
}

fn apply_public_model_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    query: &PublicGlobalModelQuery,
) {
    builder.push(" WHERE ");
    match query.is_active {
        Some(is_active) => {
            builder.push("is_active = ").push_bind(is_active);
        }
        None => {
            builder.push("is_active = TRUE");
        }
    }

    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let pattern = format!("%{search}%");
        builder
            .push(" AND (name ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR display_name ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
}

fn apply_admin_global_model_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    query: &AdminGlobalModelListQuery,
) {
    builder.push(" WHERE 1=1");
    if let Some(is_active) = query.is_active {
        builder.push(" AND gm.is_active = ").push_bind(is_active);
    }
    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let pattern = format!("%{search}%");
        builder
            .push(" AND (gm.name ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR gm.display_name ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
}

fn map_row(row: &PgRow) -> Result<StoredPublicGlobalModel, DataLayerError> {
    let supported_capabilities: Option<Value> =
        row.try_get("supported_capabilities").map_postgres_err()?;
    StoredPublicGlobalModel::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("name").map_postgres_err()?,
        row.try_get("display_name").map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
        row.try_get("default_price_per_request")
            .map_postgres_err()?,
        row.try_get("default_tiered_pricing").map_postgres_err()?,
        supported_capabilities,
        row.try_get("config").map_postgres_err()?,
        0,
    )
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

fn apply_public_catalog_model_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    provider_id: Option<&str>,
    search: Option<&str>,
) {
    builder.push(" WHERE m.is_active = TRUE AND p.is_active = TRUE");

    if let Some(provider_id) = provider_id.map(str::trim).filter(|value| !value.is_empty()) {
        builder
            .push(" AND m.provider_id = ")
            .push_bind(provider_id.to_string());
    }

    if let Some(search) = search.map(str::trim).filter(|value| !value.is_empty()) {
        let pattern = format!("%{search}%");
        builder
            .push(" AND (m.provider_model_name ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR gm.name ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR gm.display_name ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
}

fn map_public_catalog_model_row(row: &PgRow) -> Result<StoredPublicCatalogModel, DataLayerError> {
    StoredPublicCatalogModel::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("provider_name").map_postgres_err()?,
        row.try_get("provider_model_name").map_postgres_err()?,
        row.try_get("name").map_postgres_err()?,
        row.try_get("display_name").map_postgres_err()?,
        row.try_get("description").map_postgres_err()?,
        row.try_get("icon_url").map_postgres_err()?,
        row.try_get("input_price_per_1m").map_postgres_err()?,
        row.try_get("output_price_per_1m").map_postgres_err()?,
        row.try_get("cache_creation_price_per_1m")
            .map_postgres_err()?,
        row.try_get("cache_read_price_per_1m").map_postgres_err()?,
        row.try_get("supports_vision").map_postgres_err()?,
        row.try_get("supports_function_calling")
            .map_postgres_err()?,
        row.try_get("supports_streaming").map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
    )
}

fn map_admin_provider_model_row(row: &PgRow) -> Result<StoredAdminProviderModel, DataLayerError> {
    let created_at_unix_ms = row
        .try_get::<Option<i64>, _>("created_at_unix_ms")
        .map_postgres_err()?
        .map(|value| value.max(0) as u64);
    let updated_at_unix_secs = row
        .try_get::<Option<i64>, _>("updated_at_unix_secs")
        .map_postgres_err()?
        .map(|value| value.max(0) as u64);
    StoredAdminProviderModel::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("global_model_id").map_postgres_err()?,
        row.try_get("provider_model_name").map_postgres_err()?,
        row.try_get("provider_model_mappings").map_postgres_err()?,
        row.try_get("price_per_request").map_postgres_err()?,
        row.try_get("tiered_pricing").map_postgres_err()?,
        row.try_get("supports_vision").map_postgres_err()?,
        row.try_get("supports_function_calling")
            .map_postgres_err()?,
        row.try_get("supports_streaming").map_postgres_err()?,
        row.try_get("supports_extended_thinking")
            .map_postgres_err()?,
        row.try_get("supports_image_generation")
            .map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
        row.try_get("is_available").map_postgres_err()?,
        row.try_get("config").map_postgres_err()?,
        created_at_unix_ms,
        updated_at_unix_secs,
        row.try_get("global_model_name").map_postgres_err()?,
        row.try_get("global_model_display_name")
            .map_postgres_err()?,
        row.try_get("global_model_default_price_per_request")
            .map_postgres_err()?,
        row.try_get("global_model_default_tiered_pricing")
            .map_postgres_err()?,
        row.try_get("global_model_config").map_postgres_err()?,
    )
}

fn map_admin_global_model_row(row: &PgRow) -> Result<StoredAdminGlobalModel, DataLayerError> {
    let created_at_unix_ms = row
        .try_get::<Option<i64>, _>("created_at_unix_ms")
        .map_postgres_err()?
        .map(|value| value.max(0) as u64);
    let updated_at_unix_secs = row
        .try_get::<Option<i64>, _>("updated_at_unix_secs")
        .map_postgres_err()?
        .map(|value| value.max(0) as u64);
    let provider_count = row
        .try_get::<i64, _>("provider_count")
        .map_postgres_err()?
        .max(0) as u64;
    let active_provider_count = row
        .try_get::<i64, _>("active_provider_count")
        .map_postgres_err()?
        .max(0) as u64;
    let usage_count = row
        .try_get::<i64, _>("usage_count")
        .map_postgres_err()?
        .max(0) as u64;
    StoredAdminGlobalModel::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("name").map_postgres_err()?,
        row.try_get("display_name").map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
        row.try_get("default_price_per_request")
            .map_postgres_err()?,
        row.try_get("default_tiered_pricing").map_postgres_err()?,
        row.try_get("supported_capabilities").map_postgres_err()?,
        row.try_get("config").map_postgres_err()?,
        provider_count,
        active_provider_count,
        usage_count,
        created_at_unix_ms,
        updated_at_unix_secs,
    )
}

fn build_provider_id_list_query<'a>(
    prefix: &'static str,
    provider_ids: &'a [String],
    suffix: &'static str,
) -> QueryBuilder<'a, Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(prefix);
    let mut separated = builder.separated(", ");
    for provider_id in provider_ids {
        separated.push_bind(provider_id);
    }
    separated.push_unseparated(suffix);
    builder
}

fn map_provider_model_stats_row(row: &PgRow) -> Result<StoredProviderModelStats, DataLayerError> {
    StoredProviderModelStats::new(
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("total_models").map_postgres_err()?,
        row.try_get("active_models").map_postgres_err()?,
    )
}

fn map_provider_active_global_model_row(
    row: &PgRow,
) -> Result<StoredProviderActiveGlobalModel, DataLayerError> {
    StoredProviderActiveGlobalModel::new(
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("global_model_id").map_postgres_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqlxGlobalModelReadRepository;
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
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

        let pool = factory.connect_lazy().expect("pool should build");
        let _repository = SqlxGlobalModelReadRepository::new(pool);
    }
}
