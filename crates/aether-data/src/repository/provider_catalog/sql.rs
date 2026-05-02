use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use super::{
    ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery, ProviderCatalogReadRepository,
    ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyPage, StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use crate::{
    error::{postgres_error, SqlxResultExt},
    DataLayerError,
};

const LIST_PROVIDERS_BY_IDS_PREFIX: &str = r#"
SELECT
  id,
  name,
  description,
  website,
  provider_type,
  CAST(billing_type AS TEXT) AS billing_type,
  CAST(monthly_quota_usd AS DOUBLE PRECISION) AS monthly_quota_usd,
  CAST(monthly_used_usd AS DOUBLE PRECISION) AS monthly_used_usd,
  quota_reset_day,
  CAST(EXTRACT(EPOCH FROM quota_last_reset_at) AS BIGINT) AS quota_last_reset_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM quota_expires_at) AS BIGINT) AS quota_expires_at_unix_secs,
  provider_priority,
  is_active,
  keep_priority_on_conversion,
  enable_format_conversion,
  concurrent_limit,
  max_retries,
  proxy,
  request_timeout,
  stream_first_byte_timeout,
  config,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM providers
WHERE id IN (
"#;

const LIST_ENDPOINTS_BY_IDS_PREFIX: &str = r#"
SELECT
  id,
  provider_id,
  api_format,
  api_family,
  endpoint_kind,
  is_active,
  health_score,
  base_url,
  header_rules,
  body_rules,
  max_retries,
  custom_path,
  config,
  format_acceptance_config,
  proxy,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM provider_endpoints
WHERE id IN (
"#;

const LIST_ENDPOINTS_BY_IDS_PREFIX_LEGACY: &str = r#"
SELECT
  id,
  provider_id,
  api_format,
  api_family,
  endpoint_kind,
  is_active,
  base_url,
  header_rules,
  body_rules,
  max_retries,
  custom_path,
  config,
  format_acceptance_config,
  proxy,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM provider_endpoints
WHERE id IN (
"#;

const LIST_ENDPOINTS_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT
  id,
  provider_id,
  api_format,
  api_family,
  endpoint_kind,
  is_active,
  health_score,
  base_url,
  header_rules,
  body_rules,
  max_retries,
  custom_path,
  config,
  format_acceptance_config,
  proxy,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM provider_endpoints
WHERE provider_id IN (
"#;

const LIST_ENDPOINTS_BY_PROVIDER_IDS_PREFIX_LEGACY: &str = r#"
SELECT
  id,
  provider_id,
  api_format,
  api_family,
  endpoint_kind,
  is_active,
  base_url,
  header_rules,
  body_rules,
  max_retries,
  custom_path,
  config,
  format_acceptance_config,
  proxy,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM provider_endpoints
WHERE provider_id IN (
"#;

const LIST_KEYS_BY_IDS_PREFIX: &str = r#"
SELECT
  id,
  provider_id,
  name,
  auth_type,
  capabilities,
  is_active,
  api_formats,
  auth_type_by_format,
  allow_auth_channel_mismatch_formats,
  api_key,
  auth_config,
  note,
  internal_priority,
  rate_multipliers,
  global_priority_by_format,
  allowed_models,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  cache_ttl_minutes,
  max_probe_interval_minutes,
  proxy,
  fingerprint,
  rpm_limit,
  concurrent_limit,
  learned_rpm_limit,
  concurrent_429_count,
  rpm_429_count,
  EXTRACT(EPOCH FROM last_429_at)::bigint AS last_429_at_unix_secs,
  last_429_type,
  adjustment_history,
  utilization_samples,
  EXTRACT(EPOCH FROM last_probe_increase_at)::bigint AS last_probe_increase_at_unix_secs,
  last_rpm_peak,
  request_count,
  total_tokens,
  CAST(total_cost_usd AS DOUBLE PRECISION) AS total_cost_usd,
  success_count,
  error_count,
  total_response_time_ms,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  auto_fetch_models,
  EXTRACT(EPOCH FROM last_models_fetch_at)::bigint AS last_models_fetch_at_unix_secs,
  last_models_fetch_error,
  locked_models,
  model_include_patterns,
  model_exclude_patterns,
  upstream_metadata,
  EXTRACT(EPOCH FROM oauth_invalid_at)::bigint AS oauth_invalid_at_unix_secs,
  oauth_invalid_reason,
  status_snapshot,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs,
  health_by_format,
  circuit_breaker_by_format
FROM provider_api_keys
WHERE id IN (
"#;

const LIST_KEYS_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT
  id,
  provider_id,
  name,
  auth_type,
  capabilities,
  is_active,
  api_formats,
  auth_type_by_format,
  allow_auth_channel_mismatch_formats,
  api_key,
  auth_config,
  note,
  internal_priority,
  rate_multipliers,
  global_priority_by_format,
  allowed_models,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  cache_ttl_minutes,
  max_probe_interval_minutes,
  proxy,
  fingerprint,
  rpm_limit,
  concurrent_limit,
  learned_rpm_limit,
  concurrent_429_count,
  rpm_429_count,
  EXTRACT(EPOCH FROM last_429_at)::bigint AS last_429_at_unix_secs,
  last_429_type,
  adjustment_history,
  utilization_samples,
  EXTRACT(EPOCH FROM last_probe_increase_at)::bigint AS last_probe_increase_at_unix_secs,
  last_rpm_peak,
  request_count,
  total_tokens,
  CAST(total_cost_usd AS DOUBLE PRECISION) AS total_cost_usd,
  success_count,
  error_count,
  total_response_time_ms,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  auto_fetch_models,
  EXTRACT(EPOCH FROM last_models_fetch_at)::bigint AS last_models_fetch_at_unix_secs,
  last_models_fetch_error,
  locked_models,
  model_include_patterns,
  model_exclude_patterns,
  upstream_metadata,
  EXTRACT(EPOCH FROM oauth_invalid_at)::bigint AS oauth_invalid_at_unix_secs,
  oauth_invalid_reason,
  status_snapshot,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs,
  health_by_format,
  circuit_breaker_by_format
FROM provider_api_keys
WHERE provider_id IN (
"#;

const LIST_KEY_SUMMARIES_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT
  id,
  provider_id,
  COALESCE(NULLIF(name, ''), id) AS name,
  COALESCE(NULLIF(auth_type, ''), 'summary') AS auth_type,
  NULL::jsonb AS capabilities,
  is_active,
  api_formats,
  NULL::jsonb AS auth_type_by_format,
  'summary' AS api_key,
  CASE
    WHEN auth_config IS NULL THEN NULL
    ELSE '{}'::text
  END AS auth_config,
  NULL::text AS note,
  NULL::integer AS internal_priority,
  NULL::jsonb AS rate_multipliers,
  NULL::jsonb AS global_priority_by_format,
  NULL::jsonb AS allowed_models,
  NULL::bigint AS expires_at_unix_secs,
  NULL::integer AS cache_ttl_minutes,
  NULL::integer AS max_probe_interval_minutes,
  NULL::jsonb AS proxy,
  NULL::jsonb AS fingerprint,
  NULL::integer AS rpm_limit,
  NULL::integer AS concurrent_limit,
  NULL::integer AS learned_rpm_limit,
  NULL::integer AS concurrent_429_count,
  NULL::integer AS rpm_429_count,
  NULL::bigint AS last_429_at_unix_secs,
  NULL::text AS last_429_type,
  NULL::jsonb AS adjustment_history,
  NULL::jsonb AS utilization_samples,
  NULL::bigint AS last_probe_increase_at_unix_secs,
  NULL::integer AS last_rpm_peak,
  NULL::integer AS request_count,
  0::bigint AS total_tokens,
  0::double precision AS total_cost_usd,
  NULL::integer AS success_count,
  NULL::integer AS error_count,
  NULL::integer AS total_response_time_ms,
  NULL::bigint AS last_used_at_unix_secs,
  FALSE AS auto_fetch_models,
  NULL::bigint AS last_models_fetch_at_unix_secs,
  NULL::text AS last_models_fetch_error,
  NULL::jsonb AS locked_models,
  NULL::jsonb AS model_include_patterns,
  NULL::jsonb AS model_exclude_patterns,
  NULL::jsonb AS upstream_metadata,
  NULL::bigint AS oauth_invalid_at_unix_secs,
  NULL::text AS oauth_invalid_reason,
  NULL::jsonb AS status_snapshot,
  NULL::bigint AS created_at_unix_ms,
  NULL::bigint AS updated_at_unix_secs,
  health_by_format,
  NULL::jsonb AS circuit_breaker_by_format
FROM provider_api_keys
WHERE provider_id IN (
"#;

const LIST_KEY_STATS_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT
  provider_id,
  COUNT(*)::BIGINT AS total_keys,
  COUNT(*) FILTER (WHERE is_active = TRUE)::BIGINT AS active_keys
FROM provider_api_keys
WHERE provider_id IN (
"#;

#[derive(Debug, Clone)]
pub struct SqlxProviderCatalogReadRepository {
    pool: PgPool,
}

impl SqlxProviderCatalogReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            build_list_query(
                LIST_PROVIDERS_BY_IDS_PREFIX,
                provider_ids,
                " ORDER BY name ASC",
            )
            .build()
            .fetch(&self.pool),
            map_provider_row,
        )
        .await
    }

    pub async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        collect_query_rows(
            sqlx::query(
                r#"
SELECT
  id,
  name,
  description,
  website,
  provider_type,
  CAST(billing_type AS TEXT) AS billing_type,
  CAST(monthly_quota_usd AS DOUBLE PRECISION) AS monthly_quota_usd,
  CAST(monthly_used_usd AS DOUBLE PRECISION) AS monthly_used_usd,
  quota_reset_day,
  CAST(EXTRACT(EPOCH FROM quota_last_reset_at) AS BIGINT) AS quota_last_reset_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM quota_expires_at) AS BIGINT) AS quota_expires_at_unix_secs,
  provider_priority,
  is_active,
  keep_priority_on_conversion,
  enable_format_conversion,
  concurrent_limit,
  max_retries,
  proxy,
  request_timeout,
  stream_first_byte_timeout,
  config,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM providers
WHERE ($1::boolean = false OR is_active = true)
ORDER BY provider_priority ASC, name ASC
"#,
            )
            .bind(active_only)
            .fetch(&self.pool),
            map_provider_row,
        )
        .await
    }

    pub async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        if endpoint_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = match collect_query_rows(
            build_list_query(
                LIST_ENDPOINTS_BY_IDS_PREFIX,
                endpoint_ids,
                " ORDER BY api_format ASC, id ASC",
            )
            .build()
            .fetch(&self.pool),
            map_endpoint_row,
        )
        .await
        {
            Ok(rows) => rows,
            Err(error) if is_missing_endpoint_health_score_column_sql(&error) => {
                collect_query_rows(
                    build_list_query(
                        LIST_ENDPOINTS_BY_IDS_PREFIX_LEGACY,
                        endpoint_ids,
                        " ORDER BY api_format ASC, id ASC",
                    )
                    .build()
                    .fetch(&self.pool),
                    map_endpoint_row,
                )
                .await?
            }
            Err(error) => return Err(postgres_error(error)),
        };
        Ok(rows)
    }

    pub async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = match collect_query_rows(
            build_list_query(
                LIST_ENDPOINTS_BY_PROVIDER_IDS_PREFIX,
                provider_ids,
                " ORDER BY provider_id ASC, api_format ASC, id ASC",
            )
            .build()
            .fetch(&self.pool),
            map_endpoint_row,
        )
        .await
        {
            Ok(rows) => rows,
            Err(error) if is_missing_endpoint_health_score_column_sql(&error) => {
                collect_query_rows(
                    build_list_query(
                        LIST_ENDPOINTS_BY_PROVIDER_IDS_PREFIX_LEGACY,
                        provider_ids,
                        " ORDER BY provider_id ASC, api_format ASC, id ASC",
                    )
                    .build()
                    .fetch(&self.pool),
                    map_endpoint_row,
                )
                .await?
            }
            Err(error) => return Err(postgres_error(error)),
        };
        Ok(rows)
    }

    pub async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        if key_ids.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            build_list_query(
                LIST_KEYS_BY_IDS_PREFIX,
                key_ids,
                " ORDER BY name ASC, id ASC",
            )
            .build()
            .fetch(&self.pool),
            map_key_row,
        )
        .await
    }

    pub async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            build_list_query(
                LIST_KEYS_BY_PROVIDER_IDS_PREFIX,
                provider_ids,
                " ORDER BY provider_id ASC, name ASC, id ASC",
            )
            .build()
            .fetch(&self.pool),
            map_key_row,
        )
        .await
    }

    pub async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            build_list_query(
                LIST_KEY_SUMMARIES_BY_PROVIDER_IDS_PREFIX,
                provider_ids,
                " ORDER BY provider_id ASC, id ASC",
            )
            .build()
            .fetch(&self.pool),
            map_key_row,
        )
        .await
    }

    pub async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
        if query.provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider_id is empty".to_string(),
            ));
        }

        let offset = i64::try_from(query.offset).map_err(|_| {
            DataLayerError::InvalidInput(format!(
                "invalid provider catalog key offset: {}",
                query.offset
            ))
        })?;
        let limit = i64::try_from(query.limit).map_err(|_| {
            DataLayerError::InvalidInput(format!(
                "invalid provider catalog key limit: {}",
                query.limit
            ))
        })?;
        let search_pattern = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", value.to_ascii_lowercase()));
        let order_by = match query.order {
            ProviderCatalogKeyListOrder::Name => "internal_priority ASC, name ASC, id ASC",
            ProviderCatalogKeyListOrder::CreatedAt => {
                "internal_priority ASC, COALESCE(created_at, TO_TIMESTAMP(0)) ASC, id ASC"
            }
        };

        let count_row = sqlx::query(
            r#"
SELECT COUNT(*)::BIGINT AS total
FROM provider_api_keys
WHERE provider_id = $1
  AND ($2::TEXT IS NULL OR LOWER(name) LIKE $2 OR LOWER(id) LIKE $2)
  AND ($3::BOOLEAN IS NULL OR is_active = $3)
"#,
        )
        .bind(&query.provider_id)
        .bind(search_pattern.as_deref())
        .bind(query.is_active)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        let total = row_get::<i64>(&count_row, "total")?.max(0) as usize;

        let sql = format!(
            r#"
SELECT
  id,
  provider_id,
  name,
  auth_type,
  capabilities,
  is_active,
  api_formats,
  auth_type_by_format,
  api_key,
  auth_config,
  note,
  internal_priority,
  rate_multipliers,
  global_priority_by_format,
  allowed_models,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  cache_ttl_minutes,
  max_probe_interval_minutes,
  proxy,
  fingerprint,
  rpm_limit,
  concurrent_limit,
  learned_rpm_limit,
  concurrent_429_count,
  rpm_429_count,
  EXTRACT(EPOCH FROM last_429_at)::bigint AS last_429_at_unix_secs,
  last_429_type,
  adjustment_history,
  utilization_samples,
  EXTRACT(EPOCH FROM last_probe_increase_at)::bigint AS last_probe_increase_at_unix_secs,
  last_rpm_peak,
  request_count,
  total_tokens,
  CAST(total_cost_usd AS DOUBLE PRECISION) AS total_cost_usd,
  success_count,
  error_count,
  total_response_time_ms,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  auto_fetch_models,
  EXTRACT(EPOCH FROM last_models_fetch_at)::bigint AS last_models_fetch_at_unix_secs,
  last_models_fetch_error,
  locked_models,
  model_include_patterns,
  model_exclude_patterns,
  upstream_metadata,
  EXTRACT(EPOCH FROM oauth_invalid_at)::bigint AS oauth_invalid_at_unix_secs,
  oauth_invalid_reason,
  status_snapshot,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs,
  health_by_format,
  circuit_breaker_by_format
FROM provider_api_keys
WHERE provider_id = $1
  AND ($2::TEXT IS NULL OR LOWER(name) LIKE $2 OR LOWER(id) LIKE $2)
  AND ($3::BOOLEAN IS NULL OR is_active = $3)
ORDER BY {order_by}
OFFSET $4
LIMIT $5
"#,
        );
        let items = collect_query_rows(
            sqlx::query(&sql)
                .bind(&query.provider_id)
                .bind(search_pattern.as_deref())
                .bind(query.is_active)
                .bind(offset)
                .bind(limit)
                .fetch(&self.pool),
            map_key_row,
        )
        .await?;

        Ok(StoredProviderCatalogKeyPage { items, total })
    }

    pub async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            build_list_query(
                LIST_KEY_STATS_BY_PROVIDER_IDS_PREFIX,
                provider_ids,
                "\nGROUP BY provider_id\nORDER BY provider_id ASC",
            )
            .build()
            .fetch(&self.pool),
            map_key_stats_row,
        )
        .await
    }

    pub async fn update_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        if key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }
        if encrypted_api_key.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog oauth api_key is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  api_key = $2,
  auth_config = $3,
  expires_at = CASE
    WHEN $4::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($4::double precision)
  END,
  oauth_invalid_at = NULL,
  oauth_invalid_reason = NULL,
  updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(key_id)
        .bind(encrypted_api_key)
        .bind(encrypted_auth_config)
        .bind(expires_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn create_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        if provider.id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.id is empty".to_string(),
            ));
        }
        if provider.name.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.name is empty".to_string(),
            ));
        }
        if provider.provider_type.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.provider_type is empty".to_string(),
            ));
        }
        if provider
            .billing_type
            .as_deref()
            .map(str::trim)
            .is_some_and(str::is_empty)
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.billing_type is empty".to_string(),
            ));
        }

        let mut tx = self.pool.begin().await.map_postgres_err()?;

        if let Some(target_priority) = shift_existing_priorities_from {
            sqlx::query(
                r#"
UPDATE providers
SET provider_priority = provider_priority + 1
WHERE provider_priority IS NOT NULL
  AND provider_priority >= $1
"#,
            )
            .bind(target_priority)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }

        sqlx::query(
            r#"
INSERT INTO providers (
  id,
  name,
  description,
  website,
  provider_type,
  billing_type,
  monthly_quota_usd,
  monthly_used_usd,
  quota_reset_day,
  quota_last_reset_at,
  quota_expires_at,
  provider_priority,
  is_active,
  keep_priority_on_conversion,
  enable_format_conversion,
  concurrent_limit,
  max_retries,
  proxy,
  request_timeout,
  stream_first_byte_timeout,
  config,
  created_at,
  updated_at
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  CAST($6 AS providerbillingtype),
  $7,
  $8,
  $9,
  CASE
    WHEN $10::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($10::double precision)
  END,
  CASE
    WHEN $11::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($11::double precision)
  END,
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
  CASE
    WHEN $22::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($22::double precision)
  END,
  CASE
    WHEN $23::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($23::double precision)
  END
)
"#,
        )
        .bind(&provider.id)
        .bind(&provider.name)
        .bind(&provider.description)
        .bind(&provider.website)
        .bind(&provider.provider_type)
        .bind(
            provider
                .billing_type
                .clone()
                .unwrap_or_else(|| "pay_as_you_go".to_string()),
        )
        .bind(provider.monthly_quota_usd)
        .bind(provider.monthly_used_usd)
        .bind(provider.quota_reset_day.map(|value| value as i32))
        .bind(
            provider
                .quota_last_reset_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(
            provider
                .quota_expires_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(provider.provider_priority)
        .bind(provider.is_active)
        .bind(provider.keep_priority_on_conversion)
        .bind(provider.enable_format_conversion)
        .bind(provider.concurrent_limit)
        .bind(provider.max_retries)
        .bind(&provider.proxy)
        .bind(provider.request_timeout_secs)
        .bind(provider.stream_first_byte_timeout_secs)
        .bind(&provider.config)
        .bind(provider.created_at_unix_ms.map(|value| value as f64))
        .bind(provider.updated_at_unix_secs.map(|value| value as f64))
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;

        tx.commit().await.map_err(postgres_error)?;

        self.list_providers_by_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "created provider catalog provider {} could not be reloaded",
                    provider.id
                ))
            })
    }

    pub async fn update_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        if provider.id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.id is empty".to_string(),
            ));
        }
        if provider.name.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.name is empty".to_string(),
            ));
        }
        if provider.provider_type.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.provider_type is empty".to_string(),
            ));
        }
        if provider
            .billing_type
            .as_deref()
            .map(str::trim)
            .is_some_and(str::is_empty)
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider.billing_type is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
UPDATE providers
SET
  name = $2,
  description = $3,
  website = $4,
  provider_type = $5,
  billing_type = CAST($6 AS providerbillingtype),
  monthly_quota_usd = $7,
  monthly_used_usd = COALESCE($8, monthly_used_usd),
  quota_reset_day = $9,
  quota_last_reset_at = CASE
    WHEN $10::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($10::double precision)
  END,
  quota_expires_at = CASE
    WHEN $11::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($11::double precision)
  END,
  provider_priority = $12,
  is_active = $13,
  keep_priority_on_conversion = $14,
  enable_format_conversion = $15,
  concurrent_limit = $16,
  max_retries = $17,
  proxy = $18,
  request_timeout = $19,
  stream_first_byte_timeout = $20,
  config = $21,
  updated_at = CASE
    WHEN $22::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($22::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(&provider.id)
        .bind(&provider.name)
        .bind(&provider.description)
        .bind(&provider.website)
        .bind(&provider.provider_type)
        .bind(
            provider
                .billing_type
                .clone()
                .unwrap_or_else(|| "pay_as_you_go".to_string()),
        )
        .bind(provider.monthly_quota_usd)
        .bind(provider.monthly_used_usd)
        .bind(provider.quota_reset_day.map(|value| value as i32))
        .bind(
            provider
                .quota_last_reset_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(
            provider
                .quota_expires_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(provider.provider_priority)
        .bind(provider.is_active)
        .bind(provider.keep_priority_on_conversion)
        .bind(provider.enable_format_conversion)
        .bind(provider.concurrent_limit)
        .bind(provider.max_retries)
        .bind(&provider.proxy)
        .bind(provider.request_timeout_secs)
        .bind(provider.stream_first_byte_timeout_secs)
        .bind(&provider.config)
        .bind(provider.updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        if rows_affected == 0 {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog provider {} not found",
                provider.id
            )));
        }

        self.list_providers_by_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "updated provider catalog provider {} could not be reloaded",
                    provider.id
                ))
            })
    }

    pub async fn delete_provider(&self, provider_id: &str) -> Result<bool, DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
DELETE FROM providers
WHERE id = $1
"#,
        )
        .bind(provider_id)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn cleanup_deleted_provider_refs(
        &self,
        provider_id: &str,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider_id is empty".to_string(),
            ));
        }

        let mut tx = self.pool.begin().await.map_postgres_err()?;

        sqlx::query(
            "UPDATE user_preferences SET default_provider_id = NULL WHERE default_provider_id = $1",
        )
        .bind(provider_id)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        sqlx::query("UPDATE usage SET provider_id = NULL WHERE provider_id = $1")
            .bind(provider_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query("UPDATE video_tasks SET provider_id = NULL WHERE provider_id = $1")
            .bind(provider_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query("DELETE FROM request_candidates WHERE provider_id = $1")
            .bind(provider_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        for endpoint_id in endpoint_ids {
            sqlx::query(
                "UPDATE usage SET provider_endpoint_id = NULL WHERE provider_endpoint_id = $1",
            )
            .bind(endpoint_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
            sqlx::query("UPDATE video_tasks SET endpoint_id = NULL WHERE endpoint_id = $1")
                .bind(endpoint_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
            sqlx::query("DELETE FROM request_candidates WHERE endpoint_id = $1")
                .bind(endpoint_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }

        for key_id in key_ids {
            sqlx::query("DELETE FROM gemini_file_mappings WHERE key_id = $1")
                .bind(key_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
            sqlx::query(
                "UPDATE usage SET provider_api_key_id = NULL WHERE provider_api_key_id = $1",
            )
            .bind(key_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
            sqlx::query("UPDATE video_tasks SET key_id = NULL WHERE key_id = $1")
                .bind(key_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }

        sqlx::query("DELETE FROM api_key_provider_mappings WHERE provider_id = $1")
            .bind(provider_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query("DELETE FROM provider_usage_tracking WHERE provider_id = $1")
            .bind(provider_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        tx.commit().await.map_err(postgres_error)?;
        Ok(())
    }

    pub async fn clear_key_oauth_invalid_marker(
        &self,
        key_id: &str,
    ) -> Result<bool, DataLayerError> {
        if key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  oauth_invalid_at = NULL,
  oauth_invalid_reason = NULL,
  updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn create_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        if key.id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key.id is empty".to_string(),
            ));
        }
        if key.provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key.provider_id is empty".to_string(),
            ));
        }

        sqlx::query(
            r#"
INSERT INTO provider_api_keys (
  id,
  provider_id,
  api_formats,
  auth_type_by_format,
  auth_type,
  api_key,
  auth_config,
  name,
  note,
  rate_multipliers,
  internal_priority,
  global_priority_by_format,
  rpm_limit,
  concurrent_limit,
  learned_rpm_limit,
  allowed_models,
  capabilities,
  cache_ttl_minutes,
  max_probe_interval_minutes,
  auto_fetch_models,
  locked_models,
  model_include_patterns,
  model_exclude_patterns,
  proxy,
  fingerprint,
  upstream_metadata,
  expires_at,
  oauth_invalid_at,
  oauth_invalid_reason,
  status_snapshot,
  concurrent_429_count,
  rpm_429_count,
  last_429_at,
  last_429_type,
  adjustment_history,
  utilization_samples,
  last_probe_increase_at,
  last_rpm_peak,
  request_count,
  total_tokens,
  total_cost_usd,
  success_count,
  error_count,
  total_response_time_ms,
  last_used_at,
  last_models_fetch_at,
  last_models_fetch_error,
  health_by_format,
  circuit_breaker_by_format,
  is_active,
  created_at,
  updated_at,
  allow_auth_channel_mismatch_formats
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
  CASE
    WHEN $26::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($26::double precision)
  END,
  CASE
    WHEN $27::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($27::double precision)
  END,
  $28,
  $29,
  COALESCE($30, 0),
  COALESCE($31, 0),
  CASE
    WHEN $32::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($32::double precision)
  END,
  $33,
  $34,
  $35,
  CASE
    WHEN $36::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($36::double precision)
  END,
  $37,
  COALESCE($38, 0),
  COALESCE($39, 0),
  COALESCE($40, 0),
  COALESCE($41, 0),
  COALESCE($42, 0),
  COALESCE($43, 0),
  CASE
    WHEN $44::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($44::double precision)
  END,
  CASE
    WHEN $45::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($45::double precision)
  END,
  $46,
  $47,
  $48,
  $49,
  CASE
    WHEN $50::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($50::double precision)
  END,
  CASE
    WHEN $51::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($51::double precision)
  END,
  $52
)
"#,
        )
        .bind(&key.id)
        .bind(&key.provider_id)
        .bind(&key.api_formats)
        .bind(&key.auth_type_by_format)
        .bind(&key.auth_type)
        .bind(&key.encrypted_api_key)
        .bind(&key.encrypted_auth_config)
        .bind(&key.name)
        .bind(&key.note)
        .bind(&key.rate_multipliers)
        .bind(key.internal_priority)
        .bind(&key.global_priority_by_format)
        .bind(key.rpm_limit.map(|value| value as i32))
        .bind(key.concurrent_limit)
        .bind(key.learned_rpm_limit.map(|value| value as i32))
        .bind(&key.allowed_models)
        .bind(&key.capabilities)
        .bind(key.cache_ttl_minutes)
        .bind(key.max_probe_interval_minutes)
        .bind(key.auto_fetch_models)
        .bind(&key.locked_models)
        .bind(&key.model_include_patterns)
        .bind(&key.model_exclude_patterns)
        .bind(&key.proxy)
        .bind(&key.fingerprint)
        .bind(&key.upstream_metadata)
        .bind(key.expires_at_unix_secs.map(|value| value as f64))
        .bind(key.oauth_invalid_at_unix_secs.map(|value| value as f64))
        .bind(&key.oauth_invalid_reason)
        .bind(&key.status_snapshot)
        .bind(key.concurrent_429_count.map(|value| value as i32))
        .bind(key.rpm_429_count.map(|value| value as i32))
        .bind(key.last_429_at_unix_secs.map(|value| value as f64))
        .bind(&key.last_429_type)
        .bind(&key.adjustment_history)
        .bind(&key.utilization_samples)
        .bind(
            key.last_probe_increase_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(key.last_rpm_peak.map(|value| value as i32))
        .bind(key.request_count.map(|value| value as i32))
        .bind(Some(i64::try_from(key.total_tokens).map_err(|_| {
            DataLayerError::InvalidInput(format!(
                "provider catalog key.total_tokens exceeds i64: {}",
                key.total_tokens
            ))
        })?))
        .bind(key.total_cost_usd)
        .bind(key.success_count.map(|value| value as i32))
        .bind(key.error_count.map(|value| value as i32))
        .bind(key.total_response_time_ms.map(|value| value as i32))
        .bind(key.last_used_at_unix_secs.map(|value| value as f64))
        .bind(key.last_models_fetch_at_unix_secs.map(|value| value as f64))
        .bind(&key.last_models_fetch_error)
        .bind(&key.health_by_format)
        .bind(&key.circuit_breaker_by_format)
        .bind(key.is_active)
        .bind(key.created_at_unix_ms.map(|value| value as f64))
        .bind(key.updated_at_unix_secs.map(|value| value as f64))
        .bind(&key.allow_auth_channel_mismatch_formats)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;

        self.list_keys_by_ids(std::slice::from_ref(&key.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "created provider catalog key {} could not be reloaded",
                    key.id
                ))
            })
    }

    pub async fn create_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        if endpoint.id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog endpoint.id is empty".to_string(),
            ));
        }
        if endpoint.provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog endpoint.provider_id is empty".to_string(),
            ));
        }

        match sqlx::query(
            r#"
INSERT INTO provider_endpoints (
  id,
  provider_id,
  api_format,
  api_family,
  endpoint_kind,
  is_active,
  health_score,
  base_url,
  header_rules,
  body_rules,
  max_retries,
  custom_path,
  config,
  format_acceptance_config,
  proxy,
  created_at,
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
  CASE
    WHEN $16::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($16::double precision)
  END,
  CASE
    WHEN $17::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($17::double precision)
  END
)
"#,
        )
        .bind(&endpoint.id)
        .bind(&endpoint.provider_id)
        .bind(&endpoint.api_format)
        .bind(&endpoint.api_family)
        .bind(&endpoint.endpoint_kind)
        .bind(endpoint.is_active)
        .bind(endpoint.health_score)
        .bind(&endpoint.base_url)
        .bind(&endpoint.header_rules)
        .bind(&endpoint.body_rules)
        .bind(endpoint.max_retries)
        .bind(&endpoint.custom_path)
        .bind(&endpoint.config)
        .bind(&endpoint.format_acceptance_config)
        .bind(&endpoint.proxy)
        .bind(endpoint.created_at_unix_ms.map(|value| value as f64))
        .bind(endpoint.updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        {
            Ok(_) => {}
            Err(error) if is_missing_endpoint_health_score_column(&error) => {
                sqlx::query(
                    r#"
INSERT INTO provider_endpoints (
  id,
  provider_id,
  api_format,
  api_family,
  endpoint_kind,
  is_active,
  base_url,
  header_rules,
  body_rules,
  max_retries,
  custom_path,
  config,
  format_acceptance_config,
  proxy,
  created_at,
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
  CASE
    WHEN $15::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($15::double precision)
  END,
  CASE
    WHEN $16::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($16::double precision)
  END
)
"#,
                )
                .bind(&endpoint.id)
                .bind(&endpoint.provider_id)
                .bind(&endpoint.api_format)
                .bind(&endpoint.api_family)
                .bind(&endpoint.endpoint_kind)
                .bind(endpoint.is_active)
                .bind(&endpoint.base_url)
                .bind(&endpoint.header_rules)
                .bind(&endpoint.body_rules)
                .bind(endpoint.max_retries)
                .bind(&endpoint.custom_path)
                .bind(&endpoint.config)
                .bind(&endpoint.format_acceptance_config)
                .bind(&endpoint.proxy)
                .bind(endpoint.created_at_unix_ms.map(|value| value as f64))
                .bind(endpoint.updated_at_unix_secs.map(|value| value as f64))
                .execute(&self.pool)
                .await
                .map_postgres_err()?;
            }
            Err(error) => return Err(postgres_error(error)),
        }

        self.list_endpoints_by_ids(std::slice::from_ref(&endpoint.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "created provider catalog endpoint {} could not be reloaded",
                    endpoint.id
                ))
            })
    }

    pub async fn update_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        if endpoint.id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog endpoint.id is empty".to_string(),
            ));
        }
        if endpoint.provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog endpoint.provider_id is empty".to_string(),
            ));
        }

        let rows_affected = match sqlx::query(
            r#"
UPDATE provider_endpoints
SET
  provider_id = $2,
  api_format = $3,
  api_family = $4,
  endpoint_kind = $5,
  is_active = $6,
  health_score = $7,
  base_url = $8,
  header_rules = $9,
  body_rules = $10,
  max_retries = $11,
  custom_path = $12,
  config = $13,
  format_acceptance_config = $14,
  proxy = $15,
  updated_at = CASE
    WHEN $16::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($16::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(&endpoint.id)
        .bind(&endpoint.provider_id)
        .bind(&endpoint.api_format)
        .bind(&endpoint.api_family)
        .bind(&endpoint.endpoint_kind)
        .bind(endpoint.is_active)
        .bind(endpoint.health_score)
        .bind(&endpoint.base_url)
        .bind(&endpoint.header_rules)
        .bind(&endpoint.body_rules)
        .bind(endpoint.max_retries)
        .bind(&endpoint.custom_path)
        .bind(&endpoint.config)
        .bind(&endpoint.format_acceptance_config)
        .bind(&endpoint.proxy)
        .bind(endpoint.updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        {
            Ok(result) => result.rows_affected(),
            Err(error) if is_missing_endpoint_health_score_column(&error) => sqlx::query(
                r#"
UPDATE provider_endpoints
SET
  provider_id = $2,
  api_format = $3,
  api_family = $4,
  endpoint_kind = $5,
  is_active = $6,
  base_url = $7,
  header_rules = $8,
  body_rules = $9,
  max_retries = $10,
  custom_path = $11,
  config = $12,
  format_acceptance_config = $13,
  proxy = $14,
  updated_at = CASE
    WHEN $15::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($15::double precision)
  END
WHERE id = $1
"#,
            )
            .bind(&endpoint.id)
            .bind(&endpoint.provider_id)
            .bind(&endpoint.api_format)
            .bind(&endpoint.api_family)
            .bind(&endpoint.endpoint_kind)
            .bind(endpoint.is_active)
            .bind(&endpoint.base_url)
            .bind(&endpoint.header_rules)
            .bind(&endpoint.body_rules)
            .bind(endpoint.max_retries)
            .bind(&endpoint.custom_path)
            .bind(&endpoint.config)
            .bind(&endpoint.format_acceptance_config)
            .bind(&endpoint.proxy)
            .bind(endpoint.updated_at_unix_secs.map(|value| value as f64))
            .execute(&self.pool)
            .await
            .map_postgres_err()?
            .rows_affected(),
            Err(error) => return Err(postgres_error(error)),
        };

        if rows_affected == 0 {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog endpoint {} not found",
                endpoint.id
            )));
        }

        self.list_endpoints_by_ids(std::slice::from_ref(&endpoint.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "updated provider catalog endpoint {} could not be reloaded",
                    endpoint.id
                ))
            })
    }

    pub async fn delete_endpoint(&self, endpoint_id: &str) -> Result<bool, DataLayerError> {
        if endpoint_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog endpoint_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
DELETE FROM provider_endpoints
WHERE id = $1
"#,
        )
        .bind(endpoint_id)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn update_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        if key.id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key.id is empty".to_string(),
            ));
        }
        if key.provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key.provider_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  provider_id = $2,
  api_formats = $3,
  auth_type_by_format = $40,
  allow_auth_channel_mismatch_formats = $41,
  auth_type = $4,
  api_key = $5,
  auth_config = $6,
  name = $7,
  note = $8,
  rate_multipliers = $9,
  internal_priority = $10,
  global_priority_by_format = $11,
  rpm_limit = $12,
  concurrent_limit = $13,
  learned_rpm_limit = $14,
  allowed_models = $15,
  capabilities = $16,
  cache_ttl_minutes = $17,
  max_probe_interval_minutes = $18,
  auto_fetch_models = $19,
  locked_models = $20,
  model_include_patterns = $21,
  model_exclude_patterns = $22,
  proxy = $23,
  fingerprint = $24,
  upstream_metadata = $25,
  expires_at = CASE
    WHEN $39::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($39::double precision)
  END,
  oauth_invalid_at = CASE
    WHEN $26::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($26::double precision)
  END,
  oauth_invalid_reason = $27,
  status_snapshot = $28,
  concurrent_429_count = COALESCE($29, 0),
  rpm_429_count = COALESCE($30, 0),
  last_429_at = CASE
    WHEN $31::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($31::double precision)
  END,
  last_429_type = $32,
  adjustment_history = $33,
  utilization_samples = $34,
  last_probe_increase_at = CASE
    WHEN $35::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($35::double precision)
  END,
  last_rpm_peak = $36,
  is_active = $37,
  updated_at = CASE
    WHEN $38::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($38::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(&key.id)
        .bind(&key.provider_id)
        .bind(&key.api_formats)
        .bind(&key.auth_type)
        .bind(&key.encrypted_api_key)
        .bind(&key.encrypted_auth_config)
        .bind(&key.name)
        .bind(&key.note)
        .bind(&key.rate_multipliers)
        .bind(key.internal_priority)
        .bind(&key.global_priority_by_format)
        .bind(key.rpm_limit.map(|value| value as i32))
        .bind(key.concurrent_limit)
        .bind(key.learned_rpm_limit.map(|value| value as i32))
        .bind(&key.allowed_models)
        .bind(&key.capabilities)
        .bind(key.cache_ttl_minutes)
        .bind(key.max_probe_interval_minutes)
        .bind(key.auto_fetch_models)
        .bind(&key.locked_models)
        .bind(&key.model_include_patterns)
        .bind(&key.model_exclude_patterns)
        .bind(&key.proxy)
        .bind(&key.fingerprint)
        .bind(&key.upstream_metadata)
        .bind(key.oauth_invalid_at_unix_secs.map(|value| value as f64))
        .bind(&key.oauth_invalid_reason)
        .bind(&key.status_snapshot)
        .bind(key.concurrent_429_count.map(|value| value as i32))
        .bind(key.rpm_429_count.map(|value| value as i32))
        .bind(key.last_429_at_unix_secs.map(|value| value as f64))
        .bind(&key.last_429_type)
        .bind(&key.adjustment_history)
        .bind(&key.utilization_samples)
        .bind(
            key.last_probe_increase_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(key.last_rpm_peak.map(|value| value as i32))
        .bind(key.is_active)
        .bind(key.updated_at_unix_secs.map(|value| value as f64))
        .bind(key.expires_at_unix_secs.map(|value| value as f64))
        .bind(&key.auth_type_by_format)
        .bind(&key.allow_auth_channel_mismatch_formats)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        if rows_affected == 0 {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog key {} not found",
                key.id
            )));
        }

        self.list_keys_by_ids(std::slice::from_ref(&key.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "updated provider catalog key {} could not be reloaded",
                    key.id
                ))
            })
    }

    pub async fn delete_key(&self, key_id: &str) -> Result<bool, DataLayerError> {
        if key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
DELETE FROM provider_api_keys
WHERE id = $1
"#,
        )
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn update_key_upstream_metadata(
        &self,
        key_id: &str,
        upstream_metadata: Option<&serde_json::Value>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        if key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  upstream_metadata = $2,
  updated_at = CASE
    WHEN $3::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($3::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(key_id)
        .bind(upstream_metadata)
        .bind(updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn update_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, DataLayerError> {
        if key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  is_active = $2,
  health_by_format = $3,
  circuit_breaker_by_format = $4,
  updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(key_id)
        .bind(is_active)
        .bind(health_by_format)
        .bind(circuit_breaker_by_format)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();

        Ok(rows_affected > 0)
    }
}

#[async_trait]
impl ProviderCatalogReadRepository for SqlxProviderCatalogReadRepository {
    async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        Self::list_providers(self, active_only).await
    }

    async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        Self::list_providers_by_ids(self, provider_ids).await
    }

    async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        Self::list_endpoints_by_ids(self, endpoint_ids).await
    }

    async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        Self::list_endpoints_by_provider_ids(self, provider_ids).await
    }

    async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Self::list_keys_by_ids(self, key_ids).await
    }

    async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Self::list_keys_by_provider_ids(self, provider_ids).await
    }

    async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Self::list_key_summaries_by_provider_ids(self, provider_ids).await
    }

    async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
        Self::list_keys_page(self, query).await
    }

    async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        Self::list_key_stats_by_provider_ids(self, provider_ids).await
    }
}

#[async_trait]
impl ProviderCatalogWriteRepository for SqlxProviderCatalogReadRepository {
    async fn create_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        Self::create_provider(self, provider, shift_existing_priorities_from).await
    }

    async fn update_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        Self::update_provider(self, provider).await
    }

    async fn delete_provider(&self, provider_id: &str) -> Result<bool, DataLayerError> {
        Self::delete_provider(self, provider_id).await
    }

    async fn cleanup_deleted_provider_refs(
        &self,
        provider_id: &str,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        Self::cleanup_deleted_provider_refs(self, provider_id, endpoint_ids, key_ids).await
    }

    async fn create_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        self.create_endpoint(endpoint).await
    }

    async fn update_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        self.update_endpoint(endpoint).await
    }

    async fn delete_endpoint(&self, endpoint_id: &str) -> Result<bool, DataLayerError> {
        Self::delete_endpoint(self, endpoint_id).await
    }

    async fn create_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        Self::create_key(self, key).await
    }

    async fn update_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        Self::update_key(self, key).await
    }

    async fn update_key_upstream_metadata(
        &self,
        key_id: &str,
        upstream_metadata: Option<&serde_json::Value>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_upstream_metadata(self, key_id, upstream_metadata, updated_at_unix_secs)
            .await
    }

    async fn delete_key(&self, key_id: &str) -> Result<bool, DataLayerError> {
        Self::delete_key(self, key_id).await
    }

    async fn clear_key_oauth_invalid_marker(&self, key_id: &str) -> Result<bool, DataLayerError> {
        Self::clear_key_oauth_invalid_marker(self, key_id).await
    }

    async fn update_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_oauth_credentials(
            self,
            key_id,
            encrypted_api_key,
            encrypted_auth_config,
            expires_at_unix_secs,
        )
        .await
    }

    async fn update_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_health_state(
            self,
            key_id,
            is_active,
            health_by_format,
            circuit_breaker_by_format,
        )
        .await
    }
}

fn build_list_query<'a>(
    prefix: &'static str,
    ids: &'a [String],
    suffix: &'static str,
) -> QueryBuilder<'a, Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(prefix);
    let mut separated = builder.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(")");
    builder.push(suffix);
    builder
}

fn row_get<T>(row: &PgRow, column: &str) -> Result<T, DataLayerError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_postgres_err()
}

fn map_provider_row(row: &PgRow) -> Result<StoredProviderCatalogProvider, DataLayerError> {
    let quota_reset_day = row_get::<Option<i32>>(row, "quota_reset_day")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid providers.quota_reset_day: {value}"
                ))
            })
        })
        .transpose()?;
    let created_at_unix_ms = row_get::<Option<i64>>(row, "created_at_unix_ms")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid providers.created_at_unix_ms: {value}"
                ))
            })
        })
        .transpose()?;
    let updated_at_unix_secs = row_get::<Option<i64>>(row, "updated_at_unix_secs")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid providers.updated_at_unix_secs: {value}"
                ))
            })
        })
        .transpose()?;
    Ok(StoredProviderCatalogProvider::new(
        row_get(row, "id")?,
        row_get(row, "name")?,
        row_get(row, "website")?,
        row_get(row, "provider_type")?,
    )?
    .with_description(row_get(row, "description")?)
    .with_billing_fields(
        row_get(row, "billing_type")?,
        row_get(row, "monthly_quota_usd")?,
        row_get(row, "monthly_used_usd")?,
        quota_reset_day,
        row_get::<Option<i64>>(row, "quota_last_reset_at_unix_secs")?.map(|value| value as u64),
        row_get::<Option<i64>>(row, "quota_expires_at_unix_secs")?.map(|value| value as u64),
    )
    .with_routing_fields(row_get(row, "provider_priority")?)
    .with_transport_fields(
        row_get(row, "is_active")?,
        row_get(row, "keep_priority_on_conversion")?,
        row_get(row, "enable_format_conversion")?,
        row_get(row, "concurrent_limit")?,
        row_get(row, "max_retries")?,
        row_get(row, "proxy")?,
        row_get(row, "request_timeout")?,
        row_get(row, "stream_first_byte_timeout")?,
        row_get(row, "config")?,
    )
    .with_timestamps(created_at_unix_ms, updated_at_unix_secs))
}

fn map_endpoint_row(row: &PgRow) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
    let created_at_unix_ms = row_get::<Option<i64>>(row, "created_at_unix_ms")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_endpoints.created_at_unix_ms: {value}"
                ))
            })
        })
        .transpose()?;
    let updated_at_unix_secs = row_get::<Option<i64>>(row, "updated_at_unix_secs")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_endpoints.updated_at_unix_secs: {value}"
                ))
            })
        })
        .transpose()?;
    StoredProviderCatalogEndpoint::new(
        row_get(row, "id")?,
        row_get(row, "provider_id")?,
        row_get(row, "api_format")?,
        row_get(row, "api_family")?,
        row_get(row, "endpoint_kind")?,
        row_get(row, "is_active")?,
    )?
    .with_timestamps(created_at_unix_ms, updated_at_unix_secs)
    .with_health_score(
        row.try_get::<Option<f64>, _>("health_score")
            .ok()
            .flatten()
            .unwrap_or(1.0),
    )
    .with_transport_fields(
        row_get(row, "base_url")?,
        row_get(row, "header_rules")?,
        row_get(row, "body_rules")?,
        row_get(row, "max_retries")?,
        row_get(row, "custom_path")?,
        row_get(row, "config")?,
        row_get(row, "format_acceptance_config")?,
        row_get(row, "proxy")?,
    )
}

fn is_missing_endpoint_health_score_column(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "42703")
        && error
            .as_database_error()
            .map(|db| db.message().contains("health_score"))
            .unwrap_or(false)
}

fn is_missing_endpoint_health_score_column_sql(error: &DataLayerError) -> bool {
    match error {
        DataLayerError::Postgres(message) => {
            message.contains("endpoint_health_score") && message.contains("does not exist")
        }
        _ => false,
    }
}

async fn collect_query_rows<T, S>(
    mut rows: S,
    mapper: fn(&PgRow) -> Result<T, DataLayerError>,
) -> Result<Vec<T>, DataLayerError>
where
    S: futures_util::TryStream<Ok = PgRow, Error = sqlx::Error> + Unpin,
{
    let mut items = Vec::new();
    while let Some(row) = rows.try_next().await.map_postgres_err()? {
        items.push(mapper(&row)?);
    }
    Ok(items)
}

fn map_key_stats_row(row: &PgRow) -> Result<StoredProviderCatalogKeyStats, DataLayerError> {
    StoredProviderCatalogKeyStats::new(
        row_get(row, "provider_id")?,
        row_get(row, "total_keys")?,
        row_get(row, "active_keys")?,
    )
}

fn map_key_row(row: &PgRow) -> Result<StoredProviderCatalogKey, DataLayerError> {
    let rpm_limit = row_get::<Option<i32>>(row, "rpm_limit")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.rpm_limit: {value}"
                ))
            })
        })
        .transpose()?;
    let concurrent_limit = row_get::<Option<i32>>(row, "concurrent_limit")?;
    let learned_rpm_limit = row_get::<Option<i32>>(row, "learned_rpm_limit")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.learned_rpm_limit: {value}"
                ))
            })
        })
        .transpose()?;
    let concurrent_429_count = row_get::<Option<i32>>(row, "concurrent_429_count")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.concurrent_429_count: {value}"
                ))
            })
        })
        .transpose()?;
    let rpm_429_count = row_get::<Option<i32>>(row, "rpm_429_count")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.rpm_429_count: {value}"
                ))
            })
        })
        .transpose()?;
    let request_count = row_get::<Option<i32>>(row, "request_count")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.request_count: {value}"
                ))
            })
        })
        .transpose()?;
    let total_tokens = row_get::<Option<i64>>(row, "total_tokens")?
        .unwrap_or(0)
        .try_into()
        .map_err(|_| {
            DataLayerError::UnexpectedValue("invalid provider_api_keys.total_tokens".to_string())
        })?;
    let total_cost_usd = row_get::<Option<f64>>(row, "total_cost_usd")?.unwrap_or(0.0);
    if !total_cost_usd.is_finite() {
        return Err(DataLayerError::UnexpectedValue(
            "invalid provider_api_keys.total_cost_usd".to_string(),
        ));
    }
    let success_count = row_get::<Option<i32>>(row, "success_count")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.success_count: {value}"
                ))
            })
        })
        .transpose()?;
    let error_count = row_get::<Option<i32>>(row, "error_count")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.error_count: {value}"
                ))
            })
        })
        .transpose()?;
    let total_response_time_ms = row_get::<Option<i32>>(row, "total_response_time_ms")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.total_response_time_ms: {value}"
                ))
            })
        })
        .transpose()?;
    let last_probe_increase_at_unix_secs =
        row_get::<Option<i64>>(row, "last_probe_increase_at_unix_secs")?
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    DataLayerError::UnexpectedValue(format!(
                        "invalid provider_api_keys.last_probe_increase_at_unix_secs: {value}"
                    ))
                })
            })
            .transpose()?;
    let last_rpm_peak = row_get::<Option<i32>>(row, "last_rpm_peak")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.last_rpm_peak: {value}"
                ))
            })
        })
        .transpose()?;
    let last_models_fetch_at_unix_secs =
        row_get::<Option<i64>>(row, "last_models_fetch_at_unix_secs")?
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    DataLayerError::UnexpectedValue(format!(
                        "invalid provider_api_keys.last_models_fetch_at_unix_secs: {value}"
                    ))
                })
            })
            .transpose()?;
    let oauth_invalid_at_unix_secs = row_get::<Option<i64>>(row, "oauth_invalid_at_unix_secs")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.oauth_invalid_at_unix_secs: {value}"
                ))
            })
        })
        .transpose()?;
    let last_used_at_unix_secs = row_get::<Option<i64>>(row, "last_used_at_unix_secs")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.last_used_at_unix_secs: {value}"
                ))
            })
        })
        .transpose()?;
    let created_at_unix_ms = row_get::<Option<i64>>(row, "created_at_unix_ms")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.created_at_unix_ms: {value}"
                ))
            })
        })
        .transpose()?;
    let updated_at_unix_secs = row_get::<Option<i64>>(row, "updated_at_unix_secs")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.updated_at_unix_secs: {value}"
                ))
            })
        })
        .transpose()?;

    StoredProviderCatalogKey::new(
        row_get(row, "id")?,
        row_get(row, "provider_id")?,
        row_get(row, "name")?,
        row_get(row, "auth_type")?,
        row_get(row, "capabilities")?,
        row_get(row, "is_active")?,
    )?
    .with_transport_fields(
        row_get(row, "api_formats")?,
        row_get::<Option<String>>(row, "api_key")?,
        row_get(row, "auth_config")?,
        row_get(row, "rate_multipliers")?,
        row_get(row, "global_priority_by_format")?,
        row_get(row, "allowed_models")?,
        row_get::<Option<i64>>(row, "expires_at_unix_secs")?
            .and_then(|value| u64::try_from(value).ok()),
        row_get(row, "proxy")?,
        row_get(row, "fingerprint")?,
    )
    .map(|key| {
        let mut key = key
            .with_rate_limit_fields(
                rpm_limit,
                concurrent_limit,
                learned_rpm_limit,
                concurrent_429_count,
                rpm_429_count,
                row.try_get::<Option<i64>, _>("last_429_at_unix_secs")
                    .ok()
                    .flatten()
                    .and_then(|value| u64::try_from(value).ok()),
                row.try_get("adjustment_history").ok(),
                request_count,
                success_count,
            )
            .with_usage_fields(error_count, total_response_time_ms)
            .with_usage_totals(total_tokens, total_cost_usd)
            .with_health_fields(
                row.try_get("health_by_format").ok(),
                row.try_get("circuit_breaker_by_format").ok(),
            );
        key.note = row.try_get("note").ok();
        key.auth_type_by_format = row.try_get("auth_type_by_format").ok();
        key.allow_auth_channel_mismatch_formats =
            row.try_get("allow_auth_channel_mismatch_formats").ok();
        key.internal_priority = row.try_get("internal_priority").unwrap_or(50);
        key.cache_ttl_minutes = row.try_get("cache_ttl_minutes").unwrap_or(5);
        key.max_probe_interval_minutes = row.try_get("max_probe_interval_minutes").unwrap_or(32);
        key.last_429_type = row.try_get("last_429_type").ok();
        key.utilization_samples = row.try_get("utilization_samples").ok();
        key.last_probe_increase_at_unix_secs = last_probe_increase_at_unix_secs;
        key.last_rpm_peak = last_rpm_peak;
        key.last_used_at_unix_secs = last_used_at_unix_secs;
        key.auto_fetch_models = row.try_get("auto_fetch_models").unwrap_or(false);
        key.last_models_fetch_at_unix_secs = last_models_fetch_at_unix_secs;
        key.last_models_fetch_error = row.try_get("last_models_fetch_error").ok();
        key.locked_models = row.try_get("locked_models").ok();
        key.model_include_patterns = row.try_get("model_include_patterns").ok();
        key.model_exclude_patterns = row.try_get("model_exclude_patterns").ok();
        key.upstream_metadata = row.try_get("upstream_metadata").ok();
        key.oauth_invalid_at_unix_secs = oauth_invalid_at_unix_secs;
        key.oauth_invalid_reason = row.try_get("oauth_invalid_reason").ok();
        key.status_snapshot = row.try_get("status_snapshot").ok();
        key.created_at_unix_ms = created_at_unix_ms;
        key.updated_at_unix_secs = updated_at_unix_secs;
        key
    })
}

#[cfg(test)]
mod tests {
    use super::SqlxProviderCatalogReadRepository;
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
        let repository = SqlxProviderCatalogReadRepository::new(pool);
        let _ = repository.pool();
    }

    #[test]
    fn key_queries_include_usage_totals() {
        for sql in [
            super::LIST_KEYS_BY_IDS_PREFIX,
            super::LIST_KEYS_BY_PROVIDER_IDS_PREFIX,
        ] {
            assert!(sql.contains("total_tokens"));
            assert!(sql.contains("total_cost_usd"));
        }
    }

    #[test]
    fn provider_api_keys_concurrent_limit_queries_include_field() {
        for sql in [
            super::LIST_KEYS_BY_IDS_PREFIX,
            super::LIST_KEYS_BY_PROVIDER_IDS_PREFIX,
            super::LIST_KEY_SUMMARIES_BY_PROVIDER_IDS_PREFIX,
        ] {
            assert!(sql.contains("concurrent_limit"));
        }

        let source = include_str!("sql.rs");
        assert!(source.contains("concurrent_limit,"));
        assert!(source.contains("concurrent_limit = $13"));
        assert!(source.contains(".bind(key.concurrent_limit)"));
        assert!(source.contains("row_get::<Option<i32>>(row, \"concurrent_limit\")"));
    }

    #[test]
    fn provider_api_keys_concurrent_limit_schema_is_nullable_without_default() {
        let migration = include_str!(
            "../../../migrations/20260502000000_add_provider_key_auth_channel_mismatch_formats.sql"
        );
        let concurrent_limit_line = migration
            .lines()
            .find(|line| line.contains("ADD COLUMN IF NOT EXISTS concurrent_limit"))
            .expect("concurrent_limit migration line should exist")
            .to_ascii_lowercase();
        assert_eq!(
            concurrent_limit_line.trim(),
            "add column if not exists concurrent_limit integer;"
        );

        let baseline = include_str!("../../../bootstrap/20260413020000_baseline_v2.sql");
        assert!(baseline.contains("CREATE TABLE IF NOT EXISTS public.provider_api_keys"));
        assert!(baseline.contains("concurrent_limit integer,"));
    }
}
