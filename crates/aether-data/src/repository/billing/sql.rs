use async_trait::async_trait;
use sqlx::{PgPool, Row};

use super::{BillingReadRepository, StoredBillingModelContext};
use crate::{error::SqlxResultExt, DataLayerError};

const FIND_MODEL_CONTEXT_SQL: &str = r#"
SELECT
  p.id AS provider_id,
  CAST(p.billing_type AS TEXT) AS provider_billing_type,
  pak.id AS provider_api_key_id,
  pak.rate_multipliers AS provider_api_key_rate_multipliers,
  pak.cache_ttl_minutes AS provider_api_key_cache_ttl_minutes,
  gm.id AS global_model_id,
  gm.name AS global_model_name,
  gm.config AS global_model_config,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS default_price_per_request,
  gm.default_tiered_pricing AS default_tiered_pricing,
  m.id AS model_id,
  m.provider_model_name AS model_provider_model_name,
  m.config AS model_config,
  CAST(m.price_per_request AS DOUBLE PRECISION) AS model_price_per_request,
  m.tiered_pricing AS model_tiered_pricing
FROM providers p
INNER JOIN global_models gm
  ON gm.is_active = TRUE
LEFT JOIN models m
  ON m.global_model_id = gm.id
 AND m.provider_id = p.id
 AND m.is_active = TRUE
LEFT JOIN provider_api_keys pak
  ON pak.id = $3
 AND pak.provider_id = p.id
WHERE p.id = $1
  AND (
    gm.name = $2
    OR m.provider_model_name = $2
    OR (
      m.provider_model_mappings IS NOT NULL
      AND (
        m.provider_model_mappings @> jsonb_build_array(jsonb_build_object('name', $2::TEXT))
        OR m.provider_model_mappings @> jsonb_build_array(to_jsonb($2::TEXT))
        OR m.provider_model_mappings @> jsonb_build_object('name', $2::TEXT)
        OR m.provider_model_mappings = to_jsonb($2::TEXT)
      )
    )
  )
ORDER BY
  CASE
    WHEN m.provider_model_name = $2 THEN 0
    WHEN m.provider_model_mappings IS NOT NULL
      AND (
        m.provider_model_mappings @> jsonb_build_array(jsonb_build_object('name', $2::TEXT))
        OR m.provider_model_mappings @> jsonb_build_array(to_jsonb($2::TEXT))
        OR m.provider_model_mappings @> jsonb_build_object('name', $2::TEXT)
        OR m.provider_model_mappings = to_jsonb($2::TEXT)
      ) THEN 1
    WHEN gm.name = $2 THEN 2
    ELSE 3
  END ASC,
  COALESCE(m.is_available, FALSE) DESC,
  CASE
    WHEN m.tiered_pricing IS NOT NULL OR m.price_per_request IS NOT NULL THEN 0
    WHEN gm.default_tiered_pricing IS NOT NULL OR gm.default_price_per_request IS NOT NULL THEN 1
    ELSE 2
  END ASC,
  m.created_at ASC
LIMIT 1
"#;

const FIND_MODEL_CONTEXT_BY_MODEL_ID_SQL: &str = r#"
SELECT
  p.id AS provider_id,
  CAST(p.billing_type AS TEXT) AS provider_billing_type,
  pak.id AS provider_api_key_id,
  pak.rate_multipliers AS provider_api_key_rate_multipliers,
  pak.cache_ttl_minutes AS provider_api_key_cache_ttl_minutes,
  gm.id AS global_model_id,
  gm.name AS global_model_name,
  gm.config AS global_model_config,
  CAST(gm.default_price_per_request AS DOUBLE PRECISION) AS default_price_per_request,
  gm.default_tiered_pricing AS default_tiered_pricing,
  m.id AS model_id,
  m.provider_model_name AS model_provider_model_name,
  m.config AS model_config,
  CAST(m.price_per_request AS DOUBLE PRECISION) AS model_price_per_request,
  m.tiered_pricing AS model_tiered_pricing
FROM providers p
INNER JOIN models m
  ON m.id = $2
 AND m.provider_id = p.id
 AND m.is_active = TRUE
INNER JOIN global_models gm
  ON gm.id = m.global_model_id
 AND gm.is_active = TRUE
LEFT JOIN provider_api_keys pak
  ON pak.id = $3
 AND pak.provider_id = p.id
WHERE p.id = $1
LIMIT 1
"#;

#[derive(Debug, Clone)]
pub struct SqlxBillingReadRepository {
    pool: PgPool,
}

impl SqlxBillingReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn find_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let row = sqlx::query(FIND_MODEL_CONTEXT_SQL)
            .bind(provider_id)
            .bind(global_model_name)
            .bind(provider_api_key_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_row).transpose()
    }

    pub async fn find_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let row = sqlx::query(FIND_MODEL_CONTEXT_BY_MODEL_ID_SQL)
            .bind(provider_id)
            .bind(model_id)
            .bind(provider_api_key_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_row).transpose()
    }
}

#[async_trait]
impl BillingReadRepository for SqlxBillingReadRepository {
    async fn find_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        Self::find_model_context(self, provider_id, provider_api_key_id, global_model_name).await
    }

    async fn find_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        Self::find_model_context_by_model_id(self, provider_id, provider_api_key_id, model_id).await
    }
}

fn map_row(row: &sqlx::postgres::PgRow) -> Result<StoredBillingModelContext, DataLayerError> {
    StoredBillingModelContext::new(
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("provider_billing_type").map_postgres_err()?,
        row.try_get("provider_api_key_id").map_postgres_err()?,
        row.try_get("provider_api_key_rate_multipliers")
            .map_postgres_err()?,
        row.try_get::<Option<i32>, _>("provider_api_key_cache_ttl_minutes")
            .map_postgres_err()?
            .map(i64::from),
        row.try_get("global_model_id").map_postgres_err()?,
        row.try_get("global_model_name").map_postgres_err()?,
        row.try_get("global_model_config").map_postgres_err()?,
        row.try_get("default_price_per_request")
            .map_postgres_err()?,
        row.try_get("default_tiered_pricing").map_postgres_err()?,
        row.try_get("model_id").map_postgres_err()?,
        row.try_get("model_provider_model_name")
            .map_postgres_err()?,
        row.try_get("model_config").map_postgres_err()?,
        row.try_get("model_price_per_request").map_postgres_err()?,
        row.try_get("model_tiered_pricing").map_postgres_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqlxBillingReadRepository;
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
        let _repository = SqlxBillingReadRepository::new(pool);
    }
}
