use std::collections::BTreeMap;

use async_trait::async_trait;
use sqlx::{
    query::Query,
    sqlite::{SqliteArguments, SqliteRow},
    QueryBuilder, Row, Sqlite,
};

use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyAdaptiveStateUpdate, ProviderCatalogKeyHealthStateUpdate,
    ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery,
    ProviderCatalogKeyRuntimeMetadataUpdate, ProviderCatalogKeyStatusSnapshotUpdate,
    ProviderCatalogReadRepository, ProviderCatalogUpstreamMetadataNamespaceUpdate,
    ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
    StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::{sqlite_optional_real, SqlitePool};
use aether_data_query::{
    push_ci_contains_any, push_eq, push_in, push_limit_offset, push_optional_eq, SqlDialect,
    WhereClause,
};

const LIST_PROVIDERS_BY_IDS_PREFIX: &str = r#"
SELECT
  id,
  name,
  description,
  website,
  provider_type,
  billing_type,
  CAST(monthly_quota_usd AS REAL) AS monthly_quota_usd,
  CAST(monthly_used_usd AS REAL) AS monthly_used_usd,
  quota_reset_day,
  quota_last_reset_at AS quota_last_reset_at_unix_secs,
  quota_expires_at AS quota_expires_at_unix_secs,
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
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
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
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
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
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
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
  COALESCE(api_key, encrypted_key) AS api_key,
  auth_config,
  note,
  internal_priority,
  rate_multipliers,
  global_priority_by_format,
  allowed_models,
  expires_at AS expires_at_unix_secs,
  cache_ttl_minutes,
  max_probe_interval_minutes,
  proxy,
  fingerprint,
  rpm_limit,
  concurrent_limit,
  learned_rpm_limit,
  concurrent_429_count,
  rpm_429_count,
  last_429_at AS last_429_at_unix_secs,
  last_429_type,
  adjustment_history,
  utilization_samples,
  last_probe_increase_at AS last_probe_increase_at_unix_secs,
  last_rpm_peak,
  request_count,
  total_tokens,
  CAST(total_cost_usd AS REAL) AS total_cost_usd,
  success_count,
  error_count,
  total_response_time_ms,
  last_used_at AS last_used_at_unix_secs,
  auto_fetch_models,
  last_models_fetch_at AS last_models_fetch_at_unix_secs,
  last_models_fetch_error,
  locked_models,
  model_include_patterns,
  model_exclude_patterns,
  upstream_metadata,
  oauth_invalid_at AS oauth_invalid_at_unix_secs,
  oauth_invalid_reason,
  status_snapshot,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs,
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
  COALESCE(api_key, encrypted_key) AS api_key,
  auth_config,
  note,
  internal_priority,
  rate_multipliers,
  global_priority_by_format,
  allowed_models,
  expires_at AS expires_at_unix_secs,
  cache_ttl_minutes,
  max_probe_interval_minutes,
  proxy,
  fingerprint,
  rpm_limit,
  concurrent_limit,
  learned_rpm_limit,
  concurrent_429_count,
  rpm_429_count,
  last_429_at AS last_429_at_unix_secs,
  last_429_type,
  adjustment_history,
  utilization_samples,
  last_probe_increase_at AS last_probe_increase_at_unix_secs,
  last_rpm_peak,
  request_count,
  total_tokens,
  CAST(total_cost_usd AS REAL) AS total_cost_usd,
  success_count,
  error_count,
  total_response_time_ms,
  last_used_at AS last_used_at_unix_secs,
  auto_fetch_models,
  last_models_fetch_at AS last_models_fetch_at_unix_secs,
  last_models_fetch_error,
  locked_models,
  model_include_patterns,
  model_exclude_patterns,
  upstream_metadata,
  oauth_invalid_at AS oauth_invalid_at_unix_secs,
  oauth_invalid_reason,
  status_snapshot,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs,
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
  NULL AS capabilities,
  is_active,
  api_formats,
  NULL AS auth_type_by_format,
  NULL AS allow_auth_channel_mismatch_formats,
  'summary' AS api_key,
  CASE
    WHEN auth_config IS NULL THEN NULL
    ELSE '{}'
  END AS auth_config,
  NULL AS note,
  NULL AS internal_priority,
  NULL AS rate_multipliers,
  NULL AS global_priority_by_format,
  NULL AS allowed_models,
  NULL AS expires_at_unix_secs,
  NULL AS cache_ttl_minutes,
  NULL AS max_probe_interval_minutes,
  NULL AS proxy,
  NULL AS fingerprint,
  NULL AS rpm_limit,
  NULL AS concurrent_limit,
  NULL AS learned_rpm_limit,
  NULL AS concurrent_429_count,
  NULL AS rpm_429_count,
  NULL AS last_429_at_unix_secs,
  NULL AS last_429_type,
  NULL AS adjustment_history,
  NULL AS utilization_samples,
  NULL AS last_probe_increase_at_unix_secs,
  NULL AS last_rpm_peak,
  NULL AS request_count,
  0 AS total_tokens,
  0.0 AS total_cost_usd,
  NULL AS success_count,
  NULL AS error_count,
  NULL AS total_response_time_ms,
  NULL AS last_used_at_unix_secs,
  FALSE AS auto_fetch_models,
  NULL AS last_models_fetch_at_unix_secs,
  NULL AS last_models_fetch_error,
  NULL AS locked_models,
  NULL AS model_include_patterns,
  NULL AS model_exclude_patterns,
  NULL AS upstream_metadata,
  NULL AS oauth_invalid_at_unix_secs,
  NULL AS oauth_invalid_reason,
  NULL AS status_snapshot,
  NULL AS created_at_unix_ms,
  NULL AS updated_at_unix_secs,
  health_by_format,
  NULL AS circuit_breaker_by_format
FROM provider_api_keys
WHERE provider_id IN (
"#;

const LIST_KEY_STATS_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT
  provider_id,
  COUNT(*) AS total_keys,
  SUM(CASE WHEN is_active THEN 1 ELSE 0 END) AS active_keys
FROM provider_api_keys
WHERE provider_id IN (
"#;

const LIST_KEY_MAINTENANCE_SUMMARIES_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT
  id,
  provider_id,
  is_active,
  upstream_metadata
FROM provider_api_keys
WHERE provider_id IN (
"#;

#[derive(Debug, Clone)]
pub struct SqliteProviderCatalogReadRepository {
    pool: SqlitePool,
}

impl SqliteProviderCatalogReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_PROVIDERS_BY_IDS_PREFIX,
            provider_ids,
            " ORDER BY name ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_provider_row).collect()
    }

    pub async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        let mut builder =
            QueryBuilder::<Sqlite>::new(select_prefix_for_in(LIST_PROVIDERS_BY_IDS_PREFIX));
        let mut where_clause = WhereClause::new();
        if active_only {
            push_eq(&mut builder, &mut where_clause, "is_active", true);
        }
        builder.push(" ORDER BY provider_priority ASC, name ASC");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_provider_row).collect()
    }

    pub async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        if endpoint_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_ENDPOINTS_BY_IDS_PREFIX,
            endpoint_ids,
            " ORDER BY api_format ASC, id ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_endpoint_row).collect()
    }

    pub async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_ENDPOINTS_BY_PROVIDER_IDS_PREFIX,
            provider_ids,
            " ORDER BY provider_id ASC, api_format ASC, id ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_endpoint_row).collect()
    }

    pub async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        if key_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_KEYS_BY_IDS_PREFIX,
            key_ids,
            " ORDER BY name ASC, id ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_key_row).collect()
    }

    pub async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_KEYS_BY_PROVIDER_IDS_PREFIX,
            provider_ids,
            " ORDER BY provider_id ASC, name ASC, id ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_key_row).collect()
    }

    pub async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_KEY_SUMMARIES_BY_PROVIDER_IDS_PREFIX,
            provider_ids,
            " ORDER BY provider_id ASC, id ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_key_row).collect()
    }

    pub async fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_KEY_MAINTENANCE_SUMMARIES_BY_PROVIDER_IDS_PREFIX,
            provider_ids,
            " ORDER BY provider_id ASC, id ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_key_maintenance_summary_row).collect()
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
        let order_by = match query.order {
            ProviderCatalogKeyListOrder::Name => "internal_priority ASC, name ASC, id ASC",
            ProviderCatalogKeyListOrder::CreatedAt => {
                "internal_priority ASC, COALESCE(created_at, 0) ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::CreatedAtAsc => {
                "created_at IS NULL ASC, created_at ASC, name ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::CreatedAtDesc => {
                "created_at IS NULL ASC, created_at DESC, name ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::LastUsedAtAsc => {
                "last_used_at IS NULL ASC, last_used_at ASC, name ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::LastUsedAtDesc => {
                "last_used_at IS NULL ASC, last_used_at DESC, name ASC, id ASC"
            }
        };

        let mut count_builder =
            QueryBuilder::<Sqlite>::new("SELECT COUNT(*) AS total FROM provider_api_keys");
        let mut count_where = WhereClause::new();
        apply_key_page_filters(&mut count_builder, &mut count_where, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?
            .max(0) as usize;

        let mut list_builder =
            QueryBuilder::<Sqlite>::new(select_prefix_for_in(LIST_KEYS_BY_IDS_PREFIX));
        let mut list_where = WhereClause::new();
        apply_key_page_filters(&mut list_builder, &mut list_where, query);
        list_builder.push(" ORDER BY ").push(order_by);
        push_limit_offset(&mut list_builder, limit, offset);
        let rows = list_builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_key_row)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(StoredProviderCatalogKeyPage { items, total })
    }

    pub async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_list_query(
            LIST_KEY_STATS_BY_PROVIDER_IDS_PREFIX,
            provider_ids,
            "\nGROUP BY provider_id\nORDER BY provider_id ASC",
        )
        .build()
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_key_stats_row).collect()
    }

    pub async fn create_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        validate_provider(provider)?;
        let now = current_unix_secs();
        let created_at = provider.created_at_unix_ms.unwrap_or(now) as i64;
        let updated_at = provider.updated_at_unix_secs.unwrap_or(now) as i64;
        let mut tx = self.pool.begin().await.map_sql_err()?;

        if let Some(target_priority) = shift_existing_priorities_from {
            sqlx::query(
                r#"
UPDATE providers
SET provider_priority = provider_priority + 1
WHERE provider_priority >= ?
"#,
            )
            .bind(target_priority)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }

        sqlx::query(
            r#"
INSERT INTO providers (
  id, name, description, website, provider_type, billing_type,
  monthly_quota_usd, monthly_used_usd, quota_reset_day,
  quota_last_reset_at, quota_expires_at, provider_priority,
  is_active, keep_priority_on_conversion, enable_format_conversion,
  concurrent_limit, max_retries, proxy, request_timeout,
  stream_first_byte_timeout, config, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(optional_i64_from_u64(
            provider.quota_reset_day,
            "providers.quota_reset_day",
        )?)
        .bind(optional_i64_from_u64(
            provider.quota_last_reset_at_unix_secs,
            "providers.quota_last_reset_at",
        )?)
        .bind(optional_i64_from_u64(
            provider.quota_expires_at_unix_secs,
            "providers.quota_expires_at",
        )?)
        .bind(provider.provider_priority)
        .bind(provider.is_active)
        .bind(provider.keep_priority_on_conversion)
        .bind(provider.enable_format_conversion)
        .bind(provider.concurrent_limit)
        .bind(provider.max_retries)
        .bind(optional_json_to_string(&provider.proxy, "providers.proxy")?)
        .bind(provider.request_timeout_secs)
        .bind(provider.stream_first_byte_timeout_secs)
        .bind(optional_json_to_string(
            &provider.config,
            "providers.config",
        )?)
        .bind(created_at)
        .bind(updated_at)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        tx.commit().await.map_sql_err()?;
        self.reload_provider(&provider.id, "created").await
    }

    pub async fn update_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        validate_provider(provider)?;
        let updated_at = provider
            .updated_at_unix_secs
            .unwrap_or_else(current_unix_secs) as i64;
        let rows_affected = sqlx::query(
            r#"
UPDATE providers
SET
  name = ?,
  description = ?,
  website = ?,
  provider_type = ?,
  billing_type = ?,
  monthly_quota_usd = ?,
  monthly_used_usd = ?,
  quota_reset_day = ?,
  quota_last_reset_at = ?,
  quota_expires_at = ?,
  provider_priority = ?,
  is_active = ?,
  keep_priority_on_conversion = ?,
  enable_format_conversion = ?,
  concurrent_limit = ?,
  max_retries = ?,
  proxy = ?,
  request_timeout = ?,
  stream_first_byte_timeout = ?,
  config = ?,
  updated_at = ?
WHERE id = ?
"#,
        )
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
        .bind(optional_i64_from_u64(
            provider.quota_reset_day,
            "providers.quota_reset_day",
        )?)
        .bind(optional_i64_from_u64(
            provider.quota_last_reset_at_unix_secs,
            "providers.quota_last_reset_at",
        )?)
        .bind(optional_i64_from_u64(
            provider.quota_expires_at_unix_secs,
            "providers.quota_expires_at",
        )?)
        .bind(provider.provider_priority)
        .bind(provider.is_active)
        .bind(provider.keep_priority_on_conversion)
        .bind(provider.enable_format_conversion)
        .bind(provider.concurrent_limit)
        .bind(provider.max_retries)
        .bind(optional_json_to_string(&provider.proxy, "providers.proxy")?)
        .bind(provider.request_timeout_secs)
        .bind(provider.stream_first_byte_timeout_secs)
        .bind(optional_json_to_string(
            &provider.config,
            "providers.config",
        )?)
        .bind(updated_at)
        .bind(&provider.id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();

        if rows_affected == 0 {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog provider {} not found",
                provider.id
            )));
        }
        self.reload_provider(&provider.id, "updated").await
    }

    pub async fn delete_provider(&self, provider_id: &str) -> Result<bool, DataLayerError> {
        validate_non_empty(provider_id, "provider catalog provider_id")?;
        let rows_affected = sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(provider_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn cleanup_deleted_provider_refs(
        &self,
        provider_id: &str,
        provider_deleted: bool,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        validate_non_empty(provider_id, "provider catalog provider_id")?;
        let mut tx = self.pool.begin().await.map_sql_err()?;

        if provider_deleted {
            sqlx::query(
                "UPDATE user_preferences SET default_provider_id = NULL WHERE default_provider_id = ?",
            )
            .bind(provider_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            sqlx::query("UPDATE video_tasks SET provider_id = NULL WHERE provider_id = ?")
                .bind(provider_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
            sqlx::query("DELETE FROM request_candidates WHERE provider_id = ?")
                .bind(provider_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
        }

        for endpoint_id in endpoint_ids {
            sqlx::query("UPDATE video_tasks SET endpoint_id = NULL WHERE endpoint_id = ?")
                .bind(endpoint_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
            sqlx::query("DELETE FROM request_candidates WHERE endpoint_id = ?")
                .bind(endpoint_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
        }

        for key_id in key_ids {
            sqlx::query("DELETE FROM gemini_file_mappings WHERE key_id = ?")
                .bind(key_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
            sqlx::query("UPDATE video_tasks SET key_id = NULL WHERE key_id = ?")
                .bind(key_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
        }

        tx.commit().await.map_sql_err()?;
        Ok(())
    }

    pub async fn create_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        validate_endpoint(endpoint)?;
        let now = current_unix_secs();
        sqlx::query(
            r#"
INSERT INTO provider_endpoints (
  id, provider_id, name, base_url, api_format, api_family, endpoint_kind,
  is_active, health_score, header_rules, body_rules, max_retries,
  custom_path, config, format_acceptance_config, proxy, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&endpoint.id)
        .bind(&endpoint.provider_id)
        .bind(&endpoint.api_format)
        .bind(&endpoint.base_url)
        .bind(&endpoint.api_format)
        .bind(&endpoint.api_family)
        .bind(&endpoint.endpoint_kind)
        .bind(endpoint.is_active)
        .bind(endpoint.health_score)
        .bind(optional_json_to_string(
            &endpoint.header_rules,
            "provider_endpoints.header_rules",
        )?)
        .bind(optional_json_to_string(
            &endpoint.body_rules,
            "provider_endpoints.body_rules",
        )?)
        .bind(endpoint.max_retries)
        .bind(&endpoint.custom_path)
        .bind(optional_json_to_string(
            &endpoint.config,
            "provider_endpoints.config",
        )?)
        .bind(optional_json_to_string(
            &endpoint.format_acceptance_config,
            "provider_endpoints.format_acceptance_config",
        )?)
        .bind(optional_json_to_string(
            &endpoint.proxy,
            "provider_endpoints.proxy",
        )?)
        .bind(endpoint.created_at_unix_ms.unwrap_or(now) as i64)
        .bind(endpoint.updated_at_unix_secs.unwrap_or(now) as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        self.reload_endpoint(&endpoint.id, "created").await
    }

    pub async fn update_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        validate_endpoint(endpoint)?;
        let updated_at = endpoint
            .updated_at_unix_secs
            .unwrap_or_else(current_unix_secs) as i64;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_endpoints
SET
  provider_id = ?,
  name = ?,
  base_url = ?,
  api_format = ?,
  api_family = ?,
  endpoint_kind = ?,
  is_active = ?,
  health_score = ?,
  header_rules = ?,
  body_rules = ?,
  max_retries = ?,
  custom_path = ?,
  config = ?,
  format_acceptance_config = ?,
  proxy = ?,
  updated_at = ?
WHERE id = ?
"#,
        )
        .bind(&endpoint.provider_id)
        .bind(&endpoint.api_format)
        .bind(&endpoint.base_url)
        .bind(&endpoint.api_format)
        .bind(&endpoint.api_family)
        .bind(&endpoint.endpoint_kind)
        .bind(endpoint.is_active)
        .bind(endpoint.health_score)
        .bind(optional_json_to_string(
            &endpoint.header_rules,
            "provider_endpoints.header_rules",
        )?)
        .bind(optional_json_to_string(
            &endpoint.body_rules,
            "provider_endpoints.body_rules",
        )?)
        .bind(endpoint.max_retries)
        .bind(&endpoint.custom_path)
        .bind(optional_json_to_string(
            &endpoint.config,
            "provider_endpoints.config",
        )?)
        .bind(optional_json_to_string(
            &endpoint.format_acceptance_config,
            "provider_endpoints.format_acceptance_config",
        )?)
        .bind(optional_json_to_string(
            &endpoint.proxy,
            "provider_endpoints.proxy",
        )?)
        .bind(updated_at)
        .bind(&endpoint.id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();

        if rows_affected == 0 {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog endpoint {} not found",
                endpoint.id
            )));
        }
        self.reload_endpoint(&endpoint.id, "updated").await
    }

    pub async fn delete_endpoint(&self, endpoint_id: &str) -> Result<bool, DataLayerError> {
        validate_non_empty(endpoint_id, "provider catalog endpoint_id")?;
        let rows_affected = sqlx::query("DELETE FROM provider_endpoints WHERE id = ?")
            .bind(endpoint_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn create_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        validate_key(key)?;
        let now = current_unix_secs();
        sqlx::query(key_insert_sql())
            .bind(&key.id)
            .bind(&key.provider_id)
            .bind(&key.name)
            .bind(&key.encrypted_api_key)
            .bind(&key.auth_type)
            .bind(optional_json_to_string(
                &key.capabilities,
                "provider_api_keys.capabilities",
            )?)
            .bind(key.is_active)
            .bind(optional_json_to_string(
                &key.api_formats,
                "provider_api_keys.api_formats",
            )?)
            .bind(optional_json_to_string(
                &key.auth_type_by_format,
                "provider_api_keys.auth_type_by_format",
            )?)
            .bind(optional_json_to_string(
                &key.allow_auth_channel_mismatch_formats,
                "provider_api_keys.allow_auth_channel_mismatch_formats",
            )?)
            .bind(&key.encrypted_auth_config)
            .bind(&key.note)
            .bind(key.internal_priority)
            .bind(optional_json_to_string(
                &key.rate_multipliers,
                "provider_api_keys.rate_multipliers",
            )?)
            .bind(optional_json_to_string(
                &key.global_priority_by_format,
                "provider_api_keys.global_priority_by_format",
            )?)
            .bind(optional_json_to_string(
                &key.allowed_models,
                "provider_api_keys.allowed_models",
            )?)
            .bind(optional_i64_from_u64(
                key.expires_at_unix_secs,
                "provider_api_keys.expires_at",
            )?)
            .bind(key.cache_ttl_minutes)
            .bind(key.max_probe_interval_minutes)
            .bind(optional_json_to_string(
                &key.proxy,
                "provider_api_keys.proxy",
            )?)
            .bind(optional_json_to_string(
                &key.fingerprint,
                "provider_api_keys.fingerprint",
            )?)
            .bind(optional_i64_from_u32(key.rpm_limit))
            .bind(key.concurrent_limit)
            .bind(optional_i64_from_u32(key.learned_rpm_limit))
            .bind(optional_i64_from_u32(key.concurrent_429_count).unwrap_or(0))
            .bind(optional_i64_from_u32(key.rpm_429_count).unwrap_or(0))
            .bind(optional_i64_from_u64(
                key.last_429_at_unix_secs,
                "provider_api_keys.last_429_at",
            )?)
            .bind(&key.last_429_type)
            .bind(optional_json_to_string(
                &key.adjustment_history,
                "provider_api_keys.adjustment_history",
            )?)
            .bind(optional_json_to_string(
                &key.utilization_samples,
                "provider_api_keys.utilization_samples",
            )?)
            .bind(optional_i64_from_u64(
                key.last_probe_increase_at_unix_secs,
                "provider_api_keys.last_probe_increase_at",
            )?)
            .bind(optional_i64_from_u32(key.last_rpm_peak))
            .bind(optional_i64_from_u32(key.request_count).unwrap_or(0))
            .bind(i64::try_from(key.total_tokens).map_err(|_| {
                DataLayerError::InvalidInput(format!(
                    "provider catalog key.total_tokens exceeds i64: {}",
                    key.total_tokens
                ))
            })?)
            .bind(key.total_cost_usd)
            .bind(optional_i64_from_u32(key.success_count).unwrap_or(0))
            .bind(optional_i64_from_u32(key.error_count).unwrap_or(0))
            .bind(
                optional_i64_from_u64(
                    key.total_response_time_ms,
                    "provider_api_keys.total_response_time_ms",
                )?
                .unwrap_or(0),
            )
            .bind(optional_i64_from_u64(
                key.last_used_at_unix_secs,
                "provider_api_keys.last_used_at",
            )?)
            .bind(key.auto_fetch_models)
            .bind(optional_i64_from_u64(
                key.last_models_fetch_at_unix_secs,
                "provider_api_keys.last_models_fetch_at",
            )?)
            .bind(&key.last_models_fetch_error)
            .bind(optional_json_to_string(
                &key.locked_models,
                "provider_api_keys.locked_models",
            )?)
            .bind(optional_json_to_string(
                &key.model_include_patterns,
                "provider_api_keys.model_include_patterns",
            )?)
            .bind(optional_json_to_string(
                &key.model_exclude_patterns,
                "provider_api_keys.model_exclude_patterns",
            )?)
            .bind(optional_json_to_string(
                &key.upstream_metadata,
                "provider_api_keys.upstream_metadata",
            )?)
            .bind(optional_i64_from_u64(
                key.oauth_invalid_at_unix_secs,
                "provider_api_keys.oauth_invalid_at",
            )?)
            .bind(&key.oauth_invalid_reason)
            .bind(optional_json_to_string(
                &key.status_snapshot,
                "provider_api_keys.status_snapshot",
            )?)
            .bind(optional_json_to_string(
                &key.health_by_format,
                "provider_api_keys.health_by_format",
            )?)
            .bind(optional_json_to_string(
                &key.circuit_breaker_by_format,
                "provider_api_keys.circuit_breaker_by_format",
            )?)
            .bind(key.created_at_unix_ms.unwrap_or(now) as i64)
            .bind(key.updated_at_unix_secs.unwrap_or(now) as i64)
            .execute(&self.pool)
            .await
            .map_sql_err()?;

        self.reload_key(&key.id, "created").await
    }

    pub async fn update_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        validate_key(key)?;
        let updated_at = key.updated_at_unix_secs.unwrap_or_else(current_unix_secs) as i64;
        let rows_affected = key_update_query(key, updated_at)?
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();

        if rows_affected == 0 {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog key {} not found",
                key.id
            )));
        }
        self.reload_key(&key.id, "updated").await
    }

    pub async fn update_keys(
        &self,
        keys: &[StoredProviderCatalogKey],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        for key in keys {
            validate_key(key)?;
        }

        let updated_at = current_unix_secs() as i64;
        let mut transaction = self.pool.begin().await.map_sql_err()?;
        for key in keys {
            let key_updated_at = key.updated_at_unix_secs.unwrap_or(updated_at as u64) as i64;
            let rows_affected = key_update_query(key, key_updated_at)?
                .execute(&mut *transaction)
                .await
                .map_sql_err()?
                .rows_affected();
            if rows_affected == 0 {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "provider catalog key {} not found",
                    key.id
                )));
            }
        }
        transaction.commit().await.map_sql_err()?;
        let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
        let mut reloaded = self
            .list_keys_by_ids(&key_ids)
            .await?
            .into_iter()
            .map(|key| (key.id.clone(), key))
            .collect::<BTreeMap<_, _>>();
        keys.iter()
            .map(|key| {
                reloaded.remove(&key.id).ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "updated provider catalog key {} could not be reloaded",
                        key.id
                    ))
                })
            })
            .collect()
    }

    pub async fn delete_key(&self, key_id: &str) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query("DELETE FROM provider_api_keys WHERE id = ?")
            .bind(key_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_upstream_metadata(
        &self,
        key_id: &str,
        upstream_metadata: Option<&serde_json::Value>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET upstream_metadata = ?, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(optional_json_ref_to_string(
            upstream_metadata,
            "provider_api_keys.upstream_metadata",
        )?)
        .bind(updated_at_unix_secs.unwrap_or_else(current_unix_secs) as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn upsert_key_upstream_metadata_namespace(
        &self,
        key_id: &str,
        namespace: &str,
        value: &serde_json::Value,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        validate_non_empty(namespace, "provider catalog upstream metadata namespace")?;
        let value_json = serde_json::to_string(value).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
            ))
        })?;
        let namespace_path = format!(
            "$.{}",
            serde_json::to_string(namespace).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
                ))
            })?
        );
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET upstream_metadata = json_set(
      COALESCE(NULLIF(upstream_metadata, ''), '{}'),
      ?, json(?)
    ),
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(namespace_path)
        .bind(value_json)
        .bind(updated_at_unix_secs.unwrap_or_else(current_unix_secs) as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_model_fetch_state(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: Option<u64>,
        last_models_fetch_error: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET allowed_models = ?, last_models_fetch_at = ?, last_models_fetch_error = ?, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(optional_json_ref_to_string(
            allowed_models,
            "provider_api_keys.allowed_models",
        )?)
        .bind(optional_i64_from_u64(
            last_models_fetch_at_unix_secs,
            "provider_api_keys.last_models_fetch_at",
        )?)
        .bind(last_models_fetch_error)
        .bind(updated_at_unix_secs.unwrap_or_else(current_unix_secs) as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_model_fetch_success(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: u64,
        upstream_metadata_updates: &[ProviderCatalogUpstreamMetadataNamespaceUpdate],
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let allowed_models =
            optional_json_ref_to_string(allowed_models, "provider_api_keys.allowed_models")?;
        let namespace_updates = upstream_metadata_updates
            .iter()
            .map(|update| {
                validate_non_empty(
                    &update.namespace,
                    "provider catalog upstream metadata namespace",
                )?;
                let path = format!(
                    "$.{}",
                    serde_json::to_string(&update.namespace).map_err(|err| {
                        DataLayerError::UnexpectedValue(format!(
                            "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
                        ))
                    })?
                );
                let value = serde_json::to_string(&update.value).map_err(|err| {
                    DataLayerError::UnexpectedValue(format!(
                        "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
                    ))
                })?;
                Ok((path, value))
            })
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        let updated_at = updated_at_unix_secs.unwrap_or_else(current_unix_secs) as i64;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET allowed_models = ?, last_models_fetch_at = ?, last_models_fetch_error = NULL, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(allowed_models)
        .bind(optional_i64_from_u64(
            Some(last_models_fetch_at_unix_secs),
            "provider_api_keys.last_models_fetch_at",
        )?)
        .bind(updated_at)
        .bind(key_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?
        .rows_affected();
        if rows_affected == 0 {
            tx.rollback().await.map_sql_err()?;
            return Ok(false);
        }
        for (path, value) in namespace_updates {
            sqlx::query(
                r#"
UPDATE provider_api_keys
SET upstream_metadata = json_set(
      COALESCE(NULLIF(upstream_metadata, ''), '{}'),
      ?, json(?)
    )
WHERE id = ?
"#,
            )
            .bind(path)
            .bind(value)
            .bind(key_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }
        tx.commit().await.map_sql_err()?;
        Ok(true)
    }

    pub async fn clear_key_oauth_invalid_marker(
        &self,
        key_id: &str,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET oauth_invalid_at = NULL, oauth_invalid_reason = NULL, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(current_unix_secs() as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        validate_non_empty(encrypted_api_key, "provider catalog oauth api_key")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET api_key = ?, auth_config = ?, expires_at = ?, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(encrypted_api_key)
        .bind(encrypted_auth_config)
        .bind(optional_i64_from_u64(
            expires_at_unix_secs,
            "provider_api_keys.expires_at",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_oauth_runtime_state(
        &self,
        key_id: &str,
        oauth_invalid_at_unix_secs: Option<u64>,
        oauth_invalid_reason: Option<&str>,
        encrypted_auth_config_update: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET oauth_invalid_at = ?, oauth_invalid_reason = ?,
    auth_config = COALESCE(?, auth_config), updated_at = ?
WHERE id = ?
"#,
        )
        .bind(optional_i64_from_u64(
            oauth_invalid_at_unix_secs,
            "provider_api_keys.oauth_invalid_at",
        )?)
        .bind(oauth_invalid_reason)
        .bind(encrypted_auth_config_update)
        .bind(updated_at_unix_secs.unwrap_or_else(current_unix_secs) as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
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
        validate_non_empty(key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET is_active = ?, health_by_format = ?, circuit_breaker_by_format = ?, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(is_active)
        .bind(optional_json_ref_to_string(
            health_by_format,
            "provider_api_keys.health_by_format",
        )?)
        .bind(optional_json_ref_to_string(
            circuit_breaker_by_format,
            "provider_api_keys.circuit_breaker_by_format",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn reset_key_error_count(&self, key_id: &str) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET error_count = 0, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(current_unix_secs() as i64)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn compare_and_update_key_adaptive_state(
        &self,
        update: &ProviderCatalogKeyAdaptiveStateUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(&update.key_id, "provider catalog key_id")?;
        let status_snapshot_patch = adaptive_status_snapshot_patch(&update.status_snapshot_patch)?;
        let expected = update.expected.canonicalized();
        let next = update.next.canonicalized();
        let mut builder =
            QueryBuilder::<Sqlite>::new("UPDATE provider_api_keys SET learned_rpm_limit = ");
        builder
            .push_bind(optional_i64_from_u32(next.learned_rpm_limit))
            .push(", rpm_429_count = ")
            .push_bind(optional_i64_from_u32(next.rpm_429_count))
            .push(", last_429_at = ")
            .push_bind(optional_i64_from_u64(
                next.last_429_at_unix_secs,
                "provider_api_keys.last_429_at",
            )?)
            .push(", last_429_type = ")
            .push_bind(&next.last_429_type)
            .push(", adjustment_history = ")
            .push_bind(optional_json_to_string(
                &next.adjustment_history,
                "provider_api_keys.adjustment_history",
            )?)
            .push(", utilization_samples = ")
            .push_bind(optional_json_to_string(
                &next.utilization_samples,
                "provider_api_keys.utilization_samples",
            )?)
            .push(", last_probe_increase_at = ")
            .push_bind(optional_i64_from_u64(
                next.last_probe_increase_at_unix_secs,
                "provider_api_keys.last_probe_increase_at",
            )?)
            .push(", last_rpm_peak = ")
            .push_bind(optional_i64_from_u32(next.last_rpm_peak))
            .push(", concurrent_429_count = ")
            .push_bind(optional_i64_from_u32(next.concurrent_429_count))
            .push(", status_snapshot = ");
        push_status_snapshot_shallow_patch(&mut builder, &status_snapshot_patch)?;
        builder
            .push(", updated_at = ")
            .push_bind(
                update
                    .updated_at_unix_secs
                    .unwrap_or_else(current_unix_secs) as i64,
            )
            .push(" WHERE id = ")
            .push_bind(&update.key_id)
            .push(" AND learned_rpm_limit IS ")
            .push_bind(optional_i64_from_u32(expected.learned_rpm_limit))
            .push(" AND rpm_429_count IS ")
            .push_bind(optional_i64_from_u32(expected.rpm_429_count))
            .push(" AND last_429_at IS ")
            .push_bind(optional_i64_from_u64(
                expected.last_429_at_unix_secs,
                "provider_api_keys.last_429_at",
            )?)
            .push(" AND last_429_type IS ")
            .push_bind(&expected.last_429_type)
            .push(" AND json(adjustment_history) IS json(")
            .push_bind(optional_json_to_string(
                &expected.adjustment_history,
                "provider_api_keys.adjustment_history",
            )?)
            .push(")")
            .push(" AND json(utilization_samples) IS json(")
            .push_bind(optional_json_to_string(
                &expected.utilization_samples,
                "provider_api_keys.utilization_samples",
            )?)
            .push(")")
            .push(" AND last_probe_increase_at IS ")
            .push_bind(optional_i64_from_u64(
                expected.last_probe_increase_at_unix_secs,
                "provider_api_keys.last_probe_increase_at",
            )?)
            .push(" AND last_rpm_peak IS ")
            .push_bind(optional_i64_from_u32(expected.last_rpm_peak))
            .push(" AND concurrent_429_count IS ")
            .push_bind(optional_i64_from_u32(expected.concurrent_429_count));
        let rows_affected = builder
            .build()
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_runtime_metadata(
        &self,
        update: &ProviderCatalogKeyRuntimeMetadataUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_runtime_metadata_update(update)?;
        let namespace_path = format!(
            "$.{}",
            serde_json::to_string(&update.namespace).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
                ))
            })?
        );
        let metadata_value =
            serde_json::to_string(&update.upstream_metadata_value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "provider_api_keys.upstream_metadata value is not serializable: {err}"
                ))
            })?;
        let expected_metadata_value = update
            .expected_upstream_metadata_value
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "provider_api_keys.upstream_metadata expected value is not serializable: {err}"
                ))
            })?;
        let mut builder = QueryBuilder::<Sqlite>::new(
            "UPDATE provider_api_keys SET upstream_metadata = json_set(\
             COALESCE(NULLIF(upstream_metadata, ''), '{}'), ",
        );
        builder
            .push_bind(namespace_path.clone())
            .push(", json(")
            .push_bind(metadata_value)
            .push(")), status_snapshot = ");
        push_status_snapshot_shallow_patch(&mut builder, &update.status_snapshot_patch)?;
        builder
            .push(", updated_at = ")
            .push_bind(
                update
                    .updated_at_unix_secs
                    .unwrap_or_else(current_unix_secs) as i64,
            )
            .push(" WHERE id = ")
            .push_bind(&update.key_id);
        if let Some(expected_metadata_value) = expected_metadata_value {
            builder
                .push(" AND json_type(COALESCE(NULLIF(upstream_metadata, ''), '{}'), ")
                .push_bind(namespace_path.clone())
                .push(") = json_type(json(")
                .push_bind(expected_metadata_value.clone())
                .push(")) AND json_extract(COALESCE(NULLIF(upstream_metadata, ''), '{}'), ")
                .push_bind(namespace_path)
                .push(") IS json_extract(json_object('value', json(")
                .push_bind(expected_metadata_value)
                .push(")), '$.value')");
        } else {
            builder
                .push(" AND json_type(COALESCE(NULLIF(upstream_metadata, ''), '{}'), ")
                .push_bind(namespace_path)
                .push(") IS NULL");
        }
        let rows_affected = builder
            .build()
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_status_snapshot(
        &self,
        update: &ProviderCatalogKeyStatusSnapshotUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(&update.key_id, "provider catalog key_id")?;
        if !update.status_snapshot_patch.is_object() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog status snapshot patch must be an object".to_string(),
            ));
        }
        let mut builder =
            QueryBuilder::<Sqlite>::new("UPDATE provider_api_keys SET status_snapshot = ");
        push_status_snapshot_shallow_patch(&mut builder, &update.status_snapshot_patch)?;
        builder
            .push(", updated_at = ")
            .push_bind(
                update
                    .updated_at_unix_secs
                    .unwrap_or_else(current_unix_secs) as i64,
            )
            .push(" WHERE id = ")
            .push_bind(&update.key_id);
        let rows_affected = builder
            .build()
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn compare_and_update_key_health_state(
        &self,
        update: &ProviderCatalogKeyHealthStateUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(&update.key_id, "provider catalog key_id")?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET health_by_format = ?, circuit_breaker_by_format = ?, updated_at = ?
WHERE id = ?
  AND json(health_by_format) IS json(?)
  AND json(circuit_breaker_by_format) IS json(?)
"#,
        )
        .bind(optional_json_to_string(
            &update.health_by_format,
            "provider_api_keys.health_by_format",
        )?)
        .bind(optional_json_to_string(
            &update.circuit_breaker_by_format,
            "provider_api_keys.circuit_breaker_by_format",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(&update.key_id)
        .bind(optional_json_to_string(
            &update.expected_health_by_format,
            "provider_api_keys.health_by_format",
        )?)
        .bind(optional_json_to_string(
            &update.expected_circuit_breaker_by_format,
            "provider_api_keys.circuit_breaker_by_format",
        )?)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    async fn reload_provider(
        &self,
        provider_id: &str,
        operation: &str,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        self.list_providers_by_ids(&[provider_id.to_string()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "{operation} provider catalog provider {provider_id} could not be reloaded"
                ))
            })
    }

    async fn reload_endpoint(
        &self,
        endpoint_id: &str,
        operation: &str,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        self.list_endpoints_by_ids(&[endpoint_id.to_string()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "{operation} provider catalog endpoint {endpoint_id} could not be reloaded"
                ))
            })
    }

    async fn reload_key(
        &self,
        key_id: &str,
        operation: &str,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        self.list_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "{operation} provider catalog key {key_id} could not be reloaded"
                ))
            })
    }
}

#[async_trait]
impl ProviderCatalogReadRepository for SqliteProviderCatalogReadRepository {
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

    async fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
        Self::list_key_maintenance_summaries_by_provider_ids(self, provider_ids).await
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
impl ProviderCatalogWriteRepository for SqliteProviderCatalogReadRepository {
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
        provider_deleted: bool,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        Self::cleanup_deleted_provider_refs(
            self,
            provider_id,
            provider_deleted,
            endpoint_ids,
            key_ids,
        )
        .await
    }

    async fn create_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        Self::create_endpoint(self, endpoint).await
    }

    async fn update_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        Self::update_endpoint(self, endpoint).await
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

    async fn update_keys(
        &self,
        keys: &[StoredProviderCatalogKey],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Self::update_keys(self, keys).await
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

    async fn upsert_key_upstream_metadata_namespace(
        &self,
        key_id: &str,
        namespace: &str,
        value: &serde_json::Value,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        Self::upsert_key_upstream_metadata_namespace(
            self,
            key_id,
            namespace,
            value,
            updated_at_unix_secs,
        )
        .await
    }

    async fn update_key_model_fetch_state(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: Option<u64>,
        last_models_fetch_error: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_model_fetch_state(
            self,
            key_id,
            allowed_models,
            last_models_fetch_at_unix_secs,
            last_models_fetch_error,
            updated_at_unix_secs,
        )
        .await
    }

    async fn update_key_model_fetch_success(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: u64,
        upstream_metadata_updates: &[ProviderCatalogUpstreamMetadataNamespaceUpdate],
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_model_fetch_success(
            self,
            key_id,
            allowed_models,
            last_models_fetch_at_unix_secs,
            upstream_metadata_updates,
            updated_at_unix_secs,
        )
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

    async fn update_key_oauth_runtime_state(
        &self,
        key_id: &str,
        oauth_invalid_at_unix_secs: Option<u64>,
        oauth_invalid_reason: Option<&str>,
        encrypted_auth_config_update: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_oauth_runtime_state(
            self,
            key_id,
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason,
            encrypted_auth_config_update,
            updated_at_unix_secs,
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

    async fn reset_key_error_count(&self, key_id: &str) -> Result<bool, DataLayerError> {
        Self::reset_key_error_count(self, key_id).await
    }

    async fn compare_and_update_key_adaptive_state(
        &self,
        update: &ProviderCatalogKeyAdaptiveStateUpdate,
    ) -> Result<bool, DataLayerError> {
        Self::compare_and_update_key_adaptive_state(self, update).await
    }

    async fn update_key_runtime_metadata(
        &self,
        update: &ProviderCatalogKeyRuntimeMetadataUpdate,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_runtime_metadata(self, update).await
    }

    async fn update_key_status_snapshot(
        &self,
        update: &ProviderCatalogKeyStatusSnapshotUpdate,
    ) -> Result<bool, DataLayerError> {
        Self::update_key_status_snapshot(self, update).await
    }

    async fn compare_and_update_key_health_state(
        &self,
        update: &ProviderCatalogKeyHealthStateUpdate,
    ) -> Result<bool, DataLayerError> {
        Self::compare_and_update_key_health_state(self, update).await
    }
}

fn build_list_query<'a>(
    prefix: &'static str,
    ids: &'a [String],
    suffix: &'static str,
) -> QueryBuilder<'a, Sqlite> {
    let mut builder = QueryBuilder::<Sqlite>::new(select_prefix_for_in(prefix));
    let mut where_clause = WhereClause::new();
    push_in(
        &mut builder,
        &mut where_clause,
        in_column_for_prefix(prefix),
        ids,
    );
    builder.push(suffix);
    builder
}

fn select_prefix_for_in(prefix: &'static str) -> &'static str {
    prefix
        .rsplit_once("\nWHERE ")
        .map(|(select_prefix, _)| select_prefix)
        .expect("provider catalog IN query prefix must contain WHERE")
}

fn in_column_for_prefix(prefix: &'static str) -> &'static str {
    prefix
        .rsplit_once("\nWHERE ")
        .and_then(|(_, predicate)| predicate.trim().strip_suffix("IN ("))
        .map(str::trim)
        .expect("provider catalog IN query prefix must end with IN (")
}

fn apply_key_page_filters<'a>(
    builder: &mut QueryBuilder<'a, Sqlite>,
    where_clause: &mut WhereClause,
    query: &'a ProviderCatalogKeyListQuery,
) {
    push_eq(
        builder,
        where_clause,
        "provider_id",
        query.provider_id.clone(),
    );
    if let Some(search) = query.search.as_deref() {
        push_ci_contains_any(
            builder,
            where_clause,
            SqlDialect::Sqlite,
            &["name", "id"],
            search,
        );
    }
    push_optional_eq(builder, where_clause, "is_active", query.is_active);
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn validate_non_empty(value: &str, field_name: &str) -> Result<(), DataLayerError> {
    if value.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(format!(
            "{field_name} is empty"
        )));
    }
    Ok(())
}

fn adaptive_status_snapshot_patch(
    patch: &serde_json::Value,
) -> Result<serde_json::Value, DataLayerError> {
    const OWNED_FIELDS: [&str; 6] = [
        "observation_count",
        "header_observation_count",
        "latest_upstream_limit",
        "learning_confidence",
        "enforcement_active",
        "known_boundary",
    ];
    let object = patch.as_object().ok_or_else(|| {
        DataLayerError::InvalidInput(
            "provider catalog adaptive status snapshot patch must be an object".to_string(),
        )
    })?;
    Ok(serde_json::Value::Object(
        OWNED_FIELDS
            .into_iter()
            .filter_map(|field| {
                object
                    .get(field)
                    .cloned()
                    .map(|value| (field.to_string(), value))
            })
            .collect(),
    ))
}

fn validate_runtime_metadata_update(
    update: &ProviderCatalogKeyRuntimeMetadataUpdate,
) -> Result<(), DataLayerError> {
    validate_non_empty(&update.key_id, "provider catalog key_id")?;
    validate_non_empty(
        &update.namespace,
        "provider catalog runtime metadata namespace",
    )?;
    if !update.status_snapshot_patch.is_object() {
        return Err(DataLayerError::InvalidInput(
            "provider catalog runtime status snapshot patch must be an object".to_string(),
        ));
    }
    Ok(())
}

fn push_status_snapshot_shallow_patch<'args>(
    builder: &mut QueryBuilder<'args, Sqlite>,
    patch: &serde_json::Value,
) -> Result<(), DataLayerError> {
    let object = patch.as_object().ok_or_else(|| {
        DataLayerError::InvalidInput(
            "provider catalog status snapshot patch must be an object".to_string(),
        )
    })?;
    if object.is_empty() {
        builder.push("COALESCE(NULLIF(status_snapshot, ''), '{}')");
        return Ok(());
    }

    builder.push("json_set(COALESCE(NULLIF(status_snapshot, ''), '{}')");
    for (field, value) in object {
        let path = format!(
            "$.{}",
            serde_json::to_string(field).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "provider_api_keys.status_snapshot field is not serializable: {err}"
                ))
            })?
        );
        let value = serde_json::to_string(value).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "provider_api_keys.status_snapshot value is not serializable: {err}"
            ))
        })?;
        builder.push(", ").push_bind(path).push(", json(");
        builder.push_bind(value).push(")");
    }
    builder.push(")");
    Ok(())
}

fn validate_provider(provider: &StoredProviderCatalogProvider) -> Result<(), DataLayerError> {
    validate_non_empty(&provider.id, "provider catalog provider.id")?;
    validate_non_empty(&provider.name, "provider catalog provider.name")?;
    validate_non_empty(
        &provider.provider_type,
        "provider catalog provider.provider_type",
    )?;
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
    Ok(())
}

fn validate_endpoint(endpoint: &StoredProviderCatalogEndpoint) -> Result<(), DataLayerError> {
    validate_non_empty(&endpoint.id, "provider catalog endpoint.id")?;
    validate_non_empty(
        &endpoint.provider_id,
        "provider catalog endpoint.provider_id",
    )?;
    validate_non_empty(&endpoint.api_format, "provider catalog endpoint.api_format")?;
    validate_non_empty(&endpoint.base_url, "provider catalog endpoint.base_url")?;
    Ok(())
}

fn validate_key(key: &StoredProviderCatalogKey) -> Result<(), DataLayerError> {
    validate_non_empty(&key.id, "provider catalog key.id")?;
    validate_non_empty(&key.provider_id, "provider catalog key.provider_id")?;
    validate_non_empty(&key.name, "provider catalog key.name")?;
    validate_non_empty(&key.auth_type, "provider catalog key.auth_type")?;
    Ok(())
}

fn optional_i64_from_u64(
    value: Option<u64>,
    field_name: &str,
) -> Result<Option<i64>, DataLayerError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                DataLayerError::InvalidInput(format!("{field_name} exceeds i64: {value}"))
            })
        })
        .transpose()
}

fn optional_i64_from_u32(value: Option<u32>) -> Option<i64> {
    value.map(i64::from)
}

fn optional_json_ref_to_string(
    value: Option<&serde_json::Value>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains unserializable JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn optional_json_to_string(
    value: &Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    optional_json_ref_to_string(value.as_ref(), field_name)
}

fn key_insert_sql() -> &'static str {
    r#"
INSERT INTO provider_api_keys (
  id, provider_id, name, api_key, auth_type, capabilities, is_active,
  api_formats, auth_type_by_format, allow_auth_channel_mismatch_formats,
  auth_config, note, internal_priority, rate_multipliers,
  global_priority_by_format, allowed_models, expires_at, cache_ttl_minutes,
  max_probe_interval_minutes, proxy, fingerprint, rpm_limit, concurrent_limit,
  learned_rpm_limit, concurrent_429_count, rpm_429_count, last_429_at,
  last_429_type, adjustment_history, utilization_samples,
  last_probe_increase_at, last_rpm_peak, request_count, total_tokens,
  total_cost_usd, success_count, error_count, total_response_time_ms,
  last_used_at, auto_fetch_models, last_models_fetch_at,
  last_models_fetch_error, locked_models, model_include_patterns,
  model_exclude_patterns, upstream_metadata, oauth_invalid_at,
  oauth_invalid_reason, status_snapshot, health_by_format,
  circuit_breaker_by_format, created_at, updated_at
)
VALUES (
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
  ?, ?, ?, ?, ?
)
"#
}

fn key_update_sql() -> &'static str {
    r#"
UPDATE provider_api_keys
SET
  provider_id = ?,
  name = ?,
  api_key = ?,
  auth_type = ?,
  capabilities = ?,
  is_active = ?,
  api_formats = ?,
  auth_type_by_format = ?,
  allow_auth_channel_mismatch_formats = ?,
  auth_config = ?,
  note = ?,
  internal_priority = ?,
  rate_multipliers = ?,
  global_priority_by_format = ?,
  allowed_models = ?,
  expires_at = ?,
  cache_ttl_minutes = ?,
  max_probe_interval_minutes = ?,
  proxy = ?,
  fingerprint = ?,
  rpm_limit = ?,
  concurrent_limit = ?,
  auto_fetch_models = ?,
  locked_models = ?,
  model_include_patterns = ?,
  model_exclude_patterns = ?,
  updated_at = ?
WHERE id = ?
"#
}

fn key_update_query(
    key: &StoredProviderCatalogKey,
    updated_at: i64,
) -> Result<Query<'_, Sqlite, SqliteArguments<'_>>, DataLayerError> {
    Ok(sqlx::query(key_update_sql())
        .bind(&key.provider_id)
        .bind(&key.name)
        .bind(&key.encrypted_api_key)
        .bind(&key.auth_type)
        .bind(optional_json_to_string(
            &key.capabilities,
            "provider_api_keys.capabilities",
        )?)
        .bind(key.is_active)
        .bind(optional_json_to_string(
            &key.api_formats,
            "provider_api_keys.api_formats",
        )?)
        .bind(optional_json_to_string(
            &key.auth_type_by_format,
            "provider_api_keys.auth_type_by_format",
        )?)
        .bind(optional_json_to_string(
            &key.allow_auth_channel_mismatch_formats,
            "provider_api_keys.allow_auth_channel_mismatch_formats",
        )?)
        .bind(&key.encrypted_auth_config)
        .bind(&key.note)
        .bind(key.internal_priority)
        .bind(optional_json_to_string(
            &key.rate_multipliers,
            "provider_api_keys.rate_multipliers",
        )?)
        .bind(optional_json_to_string(
            &key.global_priority_by_format,
            "provider_api_keys.global_priority_by_format",
        )?)
        .bind(optional_json_to_string(
            &key.allowed_models,
            "provider_api_keys.allowed_models",
        )?)
        .bind(optional_i64_from_u64(
            key.expires_at_unix_secs,
            "provider_api_keys.expires_at",
        )?)
        .bind(key.cache_ttl_minutes)
        .bind(key.max_probe_interval_minutes)
        .bind(optional_json_to_string(
            &key.proxy,
            "provider_api_keys.proxy",
        )?)
        .bind(optional_json_to_string(
            &key.fingerprint,
            "provider_api_keys.fingerprint",
        )?)
        .bind(optional_i64_from_u32(key.rpm_limit))
        .bind(key.concurrent_limit)
        .bind(key.auto_fetch_models)
        .bind(optional_json_to_string(
            &key.locked_models,
            "provider_api_keys.locked_models",
        )?)
        .bind(optional_json_to_string(
            &key.model_include_patterns,
            "provider_api_keys.model_include_patterns",
        )?)
        .bind(optional_json_to_string(
            &key.model_exclude_patterns,
            "provider_api_keys.model_exclude_patterns",
        )?)
        .bind(updated_at)
        .bind(&key.id))
}

fn optional_json_from_string(
    value: Option<String>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains invalid JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn optional_u64(value: Option<i64>, field_name: &str) -> Result<Option<u64>, DataLayerError> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
            })
        })
        .transpose()
}

fn optional_u32(value: Option<i64>, field_name: &str) -> Result<Option<u32>, DataLayerError> {
    value
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
            })
        })
        .transpose()
}

fn map_provider_row(row: &SqliteRow) -> Result<StoredProviderCatalogProvider, DataLayerError> {
    Ok(StoredProviderCatalogProvider::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("website").map_sql_err()?,
        row.try_get("provider_type").map_sql_err()?,
    )?
    .with_description(row.try_get("description").map_sql_err()?)
    .with_billing_fields(
        row.try_get("billing_type").map_sql_err()?,
        sqlite_optional_real(row, "monthly_quota_usd")?,
        sqlite_optional_real(row, "monthly_used_usd")?,
        optional_u64(
            row.try_get("quota_reset_day").map_sql_err()?,
            "providers.quota_reset_day",
        )?,
        optional_u64(
            row.try_get("quota_last_reset_at_unix_secs").map_sql_err()?,
            "providers.quota_last_reset_at",
        )?,
        optional_u64(
            row.try_get("quota_expires_at_unix_secs").map_sql_err()?,
            "providers.quota_expires_at",
        )?,
    )
    .with_routing_fields(row.try_get("provider_priority").map_sql_err()?)
    .with_transport_fields(
        row.try_get("is_active").map_sql_err()?,
        row.try_get("keep_priority_on_conversion").map_sql_err()?,
        row.try_get("enable_format_conversion").map_sql_err()?,
        row.try_get("concurrent_limit").map_sql_err()?,
        row.try_get("max_retries").map_sql_err()?,
        optional_json_from_string(row.try_get("proxy").map_sql_err()?, "providers.proxy")?,
        row.try_get("request_timeout").map_sql_err()?,
        row.try_get("stream_first_byte_timeout").map_sql_err()?,
        optional_json_from_string(row.try_get("config").map_sql_err()?, "providers.config")?,
    )
    .with_timestamps(
        optional_u64(
            row.try_get("created_at_unix_ms").map_sql_err()?,
            "providers.created_at",
        )?,
        optional_u64(
            row.try_get("updated_at_unix_secs").map_sql_err()?,
            "providers.updated_at",
        )?,
    ))
}

fn map_endpoint_row(row: &SqliteRow) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
    StoredProviderCatalogEndpoint::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("api_format").map_sql_err()?,
        row.try_get("api_family").map_sql_err()?,
        row.try_get("endpoint_kind").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
    )?
    .with_timestamps(
        optional_u64(
            row.try_get("created_at_unix_ms").map_sql_err()?,
            "provider_endpoints.created_at",
        )?,
        optional_u64(
            row.try_get("updated_at_unix_secs").map_sql_err()?,
            "provider_endpoints.updated_at",
        )?,
    )
    .with_health_score(sqlite_optional_real(row, "health_score")?.unwrap_or(1.0))
    .with_transport_fields(
        row.try_get("base_url").map_sql_err()?,
        optional_json_from_string(
            row.try_get("header_rules").map_sql_err()?,
            "provider_endpoints.header_rules",
        )?,
        optional_json_from_string(
            row.try_get("body_rules").map_sql_err()?,
            "provider_endpoints.body_rules",
        )?,
        row.try_get("max_retries").map_sql_err()?,
        row.try_get("custom_path").map_sql_err()?,
        optional_json_from_string(
            row.try_get("config").map_sql_err()?,
            "provider_endpoints.config",
        )?,
        optional_json_from_string(
            row.try_get("format_acceptance_config").map_sql_err()?,
            "provider_endpoints.format_acceptance_config",
        )?,
        optional_json_from_string(
            row.try_get("proxy").map_sql_err()?,
            "provider_endpoints.proxy",
        )?,
    )
}

fn map_key_stats_row(row: &SqliteRow) -> Result<StoredProviderCatalogKeyStats, DataLayerError> {
    StoredProviderCatalogKeyStats::new(
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("total_keys").map_sql_err()?,
        row.try_get("active_keys").map_sql_err()?,
    )
}

fn map_key_maintenance_summary_row(
    row: &SqliteRow,
) -> Result<StoredProviderCatalogKeyMaintenanceSummary, DataLayerError> {
    Ok(StoredProviderCatalogKeyMaintenanceSummary {
        id: row.try_get("id").map_sql_err()?,
        provider_id: row.try_get("provider_id").map_sql_err()?,
        is_active: row.try_get("is_active").map_sql_err()?,
        upstream_metadata: optional_json_from_string(
            row.try_get("upstream_metadata").map_sql_err()?,
            "provider_api_keys.upstream_metadata",
        )?,
    })
}

fn map_key_row(row: &SqliteRow) -> Result<StoredProviderCatalogKey, DataLayerError> {
    let total_cost_usd = sqlite_optional_real(row, "total_cost_usd")?.unwrap_or(0.0);
    if !total_cost_usd.is_finite() {
        return Err(DataLayerError::UnexpectedValue(
            "invalid provider_api_keys.total_cost_usd".to_string(),
        ));
    }

    StoredProviderCatalogKey::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("auth_type").map_sql_err()?,
        optional_json_from_string(
            row.try_get("capabilities").map_sql_err()?,
            "provider_api_keys.capabilities",
        )?,
        row.try_get("is_active").map_sql_err()?,
    )?
    .with_transport_fields(
        optional_json_from_string(
            row.try_get("api_formats").map_sql_err()?,
            "provider_api_keys.api_formats",
        )?,
        row.try_get::<Option<String>, _>("api_key").map_sql_err()?,
        row.try_get("auth_config").map_sql_err()?,
        optional_json_from_string(
            row.try_get("rate_multipliers").map_sql_err()?,
            "provider_api_keys.rate_multipliers",
        )?,
        optional_json_from_string(
            row.try_get("global_priority_by_format").map_sql_err()?,
            "provider_api_keys.global_priority_by_format",
        )?,
        optional_json_from_string(
            row.try_get("allowed_models").map_sql_err()?,
            "provider_api_keys.allowed_models",
        )?,
        optional_u64(
            row.try_get("expires_at_unix_secs").map_sql_err()?,
            "provider_api_keys.expires_at",
        )?,
        optional_json_from_string(
            row.try_get("proxy").map_sql_err()?,
            "provider_api_keys.proxy",
        )?,
        optional_json_from_string(
            row.try_get("fingerprint").map_sql_err()?,
            "provider_api_keys.fingerprint",
        )?,
    )
    .map(|key| {
        let mut key = key
            .with_rate_limit_fields(
                optional_u32(
                    row.try_get("rpm_limit").map_sql_err()?,
                    "provider_api_keys.rpm_limit",
                )?,
                row.try_get("concurrent_limit").map_sql_err()?,
                optional_u32(
                    row.try_get("learned_rpm_limit").map_sql_err()?,
                    "provider_api_keys.learned_rpm_limit",
                )?,
                optional_u32(
                    row.try_get("concurrent_429_count").map_sql_err()?,
                    "provider_api_keys.concurrent_429_count",
                )?,
                optional_u32(
                    row.try_get("rpm_429_count").map_sql_err()?,
                    "provider_api_keys.rpm_429_count",
                )?,
                optional_u64(
                    row.try_get("last_429_at_unix_secs").map_sql_err()?,
                    "provider_api_keys.last_429_at",
                )?,
                optional_json_from_string(
                    row.try_get("adjustment_history").map_sql_err()?,
                    "provider_api_keys.adjustment_history",
                )?,
                optional_u32(
                    row.try_get("request_count").map_sql_err()?,
                    "provider_api_keys.request_count",
                )?,
                optional_u32(
                    row.try_get("success_count").map_sql_err()?,
                    "provider_api_keys.success_count",
                )?,
            )
            .with_usage_fields(
                optional_u32(
                    row.try_get("error_count").map_sql_err()?,
                    "provider_api_keys.error_count",
                )?,
                optional_u64(
                    row.try_get("total_response_time_ms").map_sql_err()?,
                    "provider_api_keys.total_response_time_ms",
                )?,
            )
            .with_usage_totals(
                optional_u64(
                    row.try_get("total_tokens").map_sql_err()?,
                    "provider_api_keys.total_tokens",
                )?
                .unwrap_or(0),
                total_cost_usd,
            )
            .with_health_fields(
                optional_json_from_string(
                    row.try_get("health_by_format").map_sql_err()?,
                    "provider_api_keys.health_by_format",
                )?,
                optional_json_from_string(
                    row.try_get("circuit_breaker_by_format").map_sql_err()?,
                    "provider_api_keys.circuit_breaker_by_format",
                )?,
            );
        key.note = row.try_get("note").map_sql_err()?;
        key.auth_type_by_format = optional_json_from_string(
            row.try_get("auth_type_by_format").map_sql_err()?,
            "provider_api_keys.auth_type_by_format",
        )?;
        key.allow_auth_channel_mismatch_formats = optional_json_from_string(
            row.try_get("allow_auth_channel_mismatch_formats")
                .map_sql_err()?,
            "provider_api_keys.allow_auth_channel_mismatch_formats",
        )?;
        key.internal_priority = row.try_get("internal_priority").map_sql_err()?;
        key.cache_ttl_minutes = row.try_get("cache_ttl_minutes").map_sql_err()?;
        key.max_probe_interval_minutes = row.try_get("max_probe_interval_minutes").map_sql_err()?;
        key.last_429_type = row.try_get("last_429_type").map_sql_err()?;
        key.utilization_samples = optional_json_from_string(
            row.try_get("utilization_samples").map_sql_err()?,
            "provider_api_keys.utilization_samples",
        )?;
        key.last_probe_increase_at_unix_secs = optional_u64(
            row.try_get("last_probe_increase_at_unix_secs")
                .map_sql_err()?,
            "provider_api_keys.last_probe_increase_at",
        )?;
        key.last_rpm_peak = optional_u32(
            row.try_get("last_rpm_peak").map_sql_err()?,
            "provider_api_keys.last_rpm_peak",
        )?;
        key.last_used_at_unix_secs = optional_u64(
            row.try_get("last_used_at_unix_secs").map_sql_err()?,
            "provider_api_keys.last_used_at",
        )?;
        key.auto_fetch_models = row.try_get("auto_fetch_models").map_sql_err()?;
        key.last_models_fetch_at_unix_secs = optional_u64(
            row.try_get("last_models_fetch_at_unix_secs")
                .map_sql_err()?,
            "provider_api_keys.last_models_fetch_at",
        )?;
        key.last_models_fetch_error = row.try_get("last_models_fetch_error").map_sql_err()?;
        key.locked_models = optional_json_from_string(
            row.try_get("locked_models").map_sql_err()?,
            "provider_api_keys.locked_models",
        )?;
        key.model_include_patterns = optional_json_from_string(
            row.try_get("model_include_patterns").map_sql_err()?,
            "provider_api_keys.model_include_patterns",
        )?;
        key.model_exclude_patterns = optional_json_from_string(
            row.try_get("model_exclude_patterns").map_sql_err()?,
            "provider_api_keys.model_exclude_patterns",
        )?;
        key.upstream_metadata = optional_json_from_string(
            row.try_get("upstream_metadata").map_sql_err()?,
            "provider_api_keys.upstream_metadata",
        )?;
        key.oauth_invalid_at_unix_secs = optional_u64(
            row.try_get("oauth_invalid_at_unix_secs").map_sql_err()?,
            "provider_api_keys.oauth_invalid_at",
        )?;
        key.oauth_invalid_reason = row.try_get("oauth_invalid_reason").map_sql_err()?;
        key.status_snapshot = optional_json_from_string(
            row.try_get("status_snapshot").map_sql_err()?,
            "provider_api_keys.status_snapshot",
        )?;
        key.created_at_unix_ms = optional_u64(
            row.try_get("created_at_unix_ms").map_sql_err()?,
            "provider_api_keys.created_at",
        )?;
        key.updated_at_unix_secs = optional_u64(
            row.try_get("updated_at_unix_secs").map_sql_err()?,
            "provider_api_keys.updated_at",
        )?;
        Ok::<_, DataLayerError>(key)
    })?
}

#[cfg(test)]
mod tests {
    use super::SqliteProviderCatalogReadRepository;
    use crate::run_migrations;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogKeyAdaptiveState, ProviderCatalogKeyAdaptiveStateUpdate,
        ProviderCatalogKeyHealthStateUpdate, ProviderCatalogKeyListOrder,
        ProviderCatalogKeyListQuery, ProviderCatalogKeyRuntimeMetadataUpdate,
        ProviderCatalogUpstreamMetadataNamespaceUpdate, StoredProviderCatalogEndpoint,
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;

    #[tokio::test]
    async fn sqlite_repository_reads_provider_catalog_contract_views() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_rows(&pool).await;

        let repository = SqliteProviderCatalogReadRepository::new(pool);
        let providers = repository
            .list_providers(true)
            .await
            .expect("providers should list");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].provider_priority, 10);
        assert_eq!(providers[0].monthly_quota_usd, Some(0.0));
        assert_eq!(providers[0].monthly_used_usd, Some(0.0));

        let endpoints = repository
            .list_endpoints_by_provider_ids(&["provider-1".to_string()])
            .await
            .expect("endpoints should list");
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].health_score, 0.95);

        let keys = repository
            .list_keys_by_provider_ids(&["provider-1".to_string()])
            .await
            .expect("keys should list");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].total_tokens, 1234);
        assert_eq!(keys[0].total_response_time_ms, Some(u32::MAX as u64 + 1));
        assert_eq!(keys[0].concurrent_limit, Some(3));

        let page = repository
            .list_keys_page(&ProviderCatalogKeyListQuery {
                provider_id: "provider-1".to_string(),
                search: Some("default".to_string()),
                is_active: Some(true),
                offset: 0,
                limit: 10,
                order: ProviderCatalogKeyListOrder::Name,
            })
            .await
            .expect("key page should load");
        assert_eq!(page.total, 1);

        let stats = repository
            .list_key_stats_by_provider_ids(&["provider-1".to_string()])
            .await
            .expect("stats should load");
        assert_eq!(stats[0].active_keys, 1);
    }

    #[tokio::test]
    async fn sqlite_runtime_key_mutations_are_field_scoped_and_compare_and_swap() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        let repository = SqliteProviderCatalogReadRepository::new(pool);
        repository
            .create_provider(
                &StoredProviderCatalogProvider::new(
                    "runtime-provider".to_string(),
                    "Runtime Provider".to_string(),
                    None,
                    "custom".to_string(),
                )
                .expect("provider should build"),
                None,
            )
            .await
            .expect("provider should create");
        let mut key = StoredProviderCatalogKey::new(
            "runtime-key".to_string(),
            "runtime-provider".to_string(),
            "Runtime Key".to_string(),
            "api_key".to_string(),
            None,
            false,
        )
        .expect("key should build")
        .with_rate_limit_fields(
            None,
            None,
            Some(10),
            None,
            Some(1),
            Some(100),
            Some(json!([{"limit":10}])),
            None,
            None,
        )
        .with_health_fields(
            Some(json!({"openai:chat":{"consecutive_failures":1}})),
            None,
        );
        key.upstream_metadata = Some(json!({
            "codex": {"remaining": 5},
            "grok": {"remaining": 7}
        }));
        key.status_snapshot = Some(json!({
            "quota": {"remaining": 5, "window": "day"},
            "oauth": {"invalid": false},
            "observation_count": 1,
            "known_boundary": "old"
        }));
        let mut stale_admin_key = key.clone();
        repository
            .create_key(&key)
            .await
            .expect("key should create");

        let health_update = ProviderCatalogKeyHealthStateUpdate {
            key_id: "runtime-key".to_string(),
            expected_health_by_format: key.health_by_format.clone(),
            expected_circuit_breaker_by_format: None,
            health_by_format: Some(json!({"openai:chat":{"consecutive_failures":2}})),
            circuit_breaker_by_format: None,
        };
        assert!(repository
            .compare_and_update_key_health_state(&health_update)
            .await
            .expect("health CAS should succeed"));
        assert!(!repository
            .compare_and_update_key_health_state(&health_update)
            .await
            .expect("stale health CAS should conflict"));

        let adaptive_current = repository
            .list_keys_by_ids(&["runtime-key".to_string()])
            .await
            .expect("key should reload before adaptive CAS")
            .pop()
            .expect("key should exist");
        let expected = ProviderCatalogKeyAdaptiveState::from(&adaptive_current);
        let mut next = expected.clone();
        next.learned_rpm_limit = Some(8);
        next.rpm_429_count = Some(2);
        assert!(repository
            .compare_and_update_key_adaptive_state(&ProviderCatalogKeyAdaptiveStateUpdate {
                key_id: "runtime-key".to_string(),
                expected: expected.clone(),
                next,
                status_snapshot_patch: json!({
                    "observation_count": 2,
                    "learning_confidence": 0.5,
                    "known_boundary": null,
                    "quota": {"remaining": 0}
                }),
                updated_at_unix_secs: Some(200),
            })
            .await
            .expect("adaptive CAS should succeed"));
        assert!(!repository
            .compare_and_update_key_adaptive_state(&ProviderCatalogKeyAdaptiveStateUpdate {
                key_id: "runtime-key".to_string(),
                expected: expected.clone(),
                next: expected,
                status_snapshot_patch: json!({}),
                updated_at_unix_secs: Some(201),
            })
            .await
            .expect("stale adaptive CAS should conflict"));

        assert!(repository
            .update_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
                key_id: "runtime-key".to_string(),
                namespace: "codex".to_string(),
                expected_upstream_metadata_value: Some(json!({"remaining":5})),
                upstream_metadata_value: json!({"remaining":3}),
                status_snapshot_patch: json!({"quota":{"remaining":3}}),
                updated_at_unix_secs: Some(300),
            })
            .await
            .expect("runtime metadata should update"));

        stale_admin_key.name = "Admin Renamed".to_string();
        stale_admin_key.is_active = true;
        repository
            .update_key(&stale_admin_key)
            .await
            .expect("stale admin update should preserve runtime fields");

        let stored = repository
            .list_keys_by_ids(&["runtime-key".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.name, "Admin Renamed");
        assert!(stored.is_active);
        assert_eq!(stored.learned_rpm_limit, Some(8));
        assert_eq!(stored.rpm_429_count, Some(2));
        assert_eq!(
            stored.health_by_format,
            Some(json!({"openai:chat":{"consecutive_failures":2}}))
        );
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["codex"],
            json!({"remaining":3})
        );
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["grok"],
            json!({"remaining":7})
        );
        let status = stored.status_snapshot.expect("status should exist");
        assert_eq!(status["quota"], json!({"remaining":3}));
        assert!(status["quota"].get("window").is_none());
        assert_eq!(status["oauth"], json!({"invalid":false}));
        assert_eq!(status["observation_count"], json!(2));
        assert_eq!(status["learning_confidence"], json!(0.5));
        assert!(status
            .as_object()
            .expect("status should be an object")
            .contains_key("known_boundary"));
        assert_eq!(status["known_boundary"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn sqlite_repository_writes_provider_catalog_contract_views() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let repository = SqliteProviderCatalogReadRepository::new(pool);
        let provider = StoredProviderCatalogProvider::new(
            "provider-write-1".to_string(),
            "Provider Write".to_string(),
            Some("https://write.example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_description(Some("write provider".to_string()))
        .with_billing_fields(
            Some("pay_as_you_go".to_string()),
            Some(100.0),
            Some(7.5),
            Some(1),
            Some(1_710_000_000),
            None,
        )
        .with_routing_fields(20)
        .with_transport_fields(
            true,
            true,
            true,
            Some(4),
            Some(2),
            Some(json!({"http":"proxy"})),
            Some(30.0),
            Some(2.5),
            Some(json!({"region":"us"})),
        );
        let created_provider = repository
            .create_provider(&provider, None)
            .await
            .expect("provider should create");
        assert_eq!(created_provider.provider_priority, 20);
        assert_eq!(created_provider.proxy, Some(json!({"http":"proxy"})));

        let mut updated_provider = created_provider.clone();
        updated_provider.description = Some("updated provider".to_string());
        updated_provider.provider_priority = 30;
        updated_provider.is_active = false;
        let updated_provider = repository
            .update_provider(&updated_provider)
            .await
            .expect("provider should update");
        assert_eq!(
            updated_provider.description,
            Some("updated provider".to_string())
        );
        assert!(!updated_provider.is_active);

        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-write-1".to_string(),
            "provider-write-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_health_score(0.88)
        .with_transport_fields(
            "https://write.example.com/v1".to_string(),
            Some(json!({"Authorization":"Bearer"})),
            Some(json!({"model":"gpt"})),
            Some(3),
            Some("/chat/completions".to_string()),
            Some(json!({"timeout":30})),
            Some(json!({"accept":["openai:chat"]})),
            Some(json!({"https":"proxy"})),
        )
        .expect("endpoint transport should build");
        let created_endpoint = repository
            .create_endpoint(&endpoint)
            .await
            .expect("endpoint should create");
        assert_eq!(created_endpoint.health_score, 0.88);

        let mut updated_endpoint = created_endpoint.clone();
        updated_endpoint.health_score = 0.5;
        updated_endpoint.is_active = false;
        let updated_endpoint = repository
            .update_endpoint(&updated_endpoint)
            .await
            .expect("endpoint should update");
        assert_eq!(updated_endpoint.health_score, 0.5);
        assert!(!updated_endpoint.is_active);

        let mut key = StoredProviderCatalogKey::new(
            "key-write-1".to_string(),
            "provider-write-1".to_string(),
            "Default Key".to_string(),
            "api_key".to_string(),
            Some(json!({"cache_1h":true})),
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:chat"])),
            Some("enc-key".to_string()),
            Some("enc-auth".to_string()),
            Some(json!({"openai:chat":1.0})),
            Some(json!({"openai:chat":10})),
            Some(json!(["gpt-4.1"])),
            Some(1_730_000_000),
            Some(json!({"http":"proxy"})),
            Some(json!({"fp":"abc"})),
        )
        .expect("key transport should build")
        .with_rate_limit_fields(
            Some(120),
            Some(3),
            Some(110),
            Some(1),
            Some(2),
            Some(1_720_000_000),
            Some(json!([{"rpm":110}])),
            Some(10),
            Some(9),
        )
        .with_usage_fields(Some(1), Some(u32::MAX as u64 + 42))
        .with_usage_totals(1234, 1.5)
        .with_health_fields(
            Some(json!({"openai:chat":{"score":1}})),
            Some(json!({"openai:chat":{"open":false}})),
        );
        key.last_models_fetch_at_unix_secs = Some(1_730_000_100);
        key.last_models_fetch_error = Some("stale models fetch error".to_string());
        let created_key = repository
            .create_key(&key)
            .await
            .expect("key should create");
        assert_eq!(created_key.concurrent_limit, Some(3));
        assert_eq!(created_key.total_tokens, 1234);
        assert_eq!(
            created_key.total_response_time_ms,
            Some(u32::MAX as u64 + 42)
        );
        assert_eq!(
            created_key.last_models_fetch_error.as_deref(),
            Some("stale models fetch error")
        );
        let mut second_key = key.clone();
        second_key.id = "key-write-2".to_string();
        second_key.name = "Secondary Key".to_string();
        let created_second_key = repository
            .create_key(&second_key)
            .await
            .expect("second key should create");

        let mut updated_key = created_key.clone();
        updated_key.name = "Updated Key".to_string();
        updated_key.is_active = false;
        updated_key.upstream_metadata = Some(json!({"models":["gpt-4.1"]}));
        updated_key.last_models_fetch_at_unix_secs = Some(1_730_000_200);
        updated_key.last_models_fetch_error = None;
        let updated_key = repository
            .update_key(&updated_key)
            .await
            .expect("key should update");
        assert_eq!(updated_key.name, "Updated Key");
        assert!(!updated_key.is_active);
        assert_eq!(
            updated_key.last_models_fetch_at_unix_secs,
            Some(1_730_000_100)
        );
        assert_eq!(
            updated_key.last_models_fetch_error.as_deref(),
            Some("stale models fetch error")
        );
        assert_eq!(updated_key.upstream_metadata, created_key.upstream_metadata);

        let mut batch_first = updated_key.clone();
        batch_first.auto_fetch_models = true;
        batch_first.allowed_models = Some(json!(["gpt-4.1", "gpt-4.1-mini"]));
        batch_first.locked_models = Some(json!(["gpt-4.1"]));
        batch_first.model_include_patterns = Some(json!(["gpt-*"]));
        batch_first.model_exclude_patterns = Some(json!(["*-preview"]));
        let mut batch_second = created_second_key;
        batch_second.auto_fetch_models = true;
        batch_second.allowed_models = batch_first.allowed_models.clone();
        batch_second.locked_models = batch_first.locked_models.clone();
        batch_second.model_include_patterns = batch_first.model_include_patterns.clone();
        batch_second.model_exclude_patterns = batch_first.model_exclude_patterns.clone();

        let batch_updated = repository
            .update_keys(&[batch_first, batch_second])
            .await
            .expect("keys should update in one transaction");
        assert_eq!(batch_updated.len(), 2);
        assert!(batch_updated.iter().all(|key| key.auto_fetch_models));
        assert!(batch_updated
            .iter()
            .all(|key| key.locked_models == Some(json!(["gpt-4.1"]))));
        assert!(batch_updated
            .iter()
            .all(|key| key.model_include_patterns == Some(json!(["gpt-*"]))));
        assert!(batch_updated
            .iter()
            .all(|key| key.model_exclude_patterns == Some(json!(["*-preview"]))));

        let mut valid_change = batch_updated
            .iter()
            .find(|key| key.id == "key-write-1")
            .expect("first key should be returned")
            .clone();
        valid_change.name = "Must Roll Back".to_string();
        let mut missing_change = valid_change.clone();
        missing_change.id = "missing-key".to_string();
        assert!(repository
            .update_keys(&[valid_change, missing_change])
            .await
            .is_err());
        let rolled_back = repository
            .list_keys_by_ids(&["key-write-1".to_string()])
            .await
            .expect("first key should reload")
            .pop()
            .expect("first key should exist");
        assert_eq!(rolled_back.name, "Updated Key");

        assert!(repository
            .update_key_upstream_metadata(
                "key-write-1",
                Some(&json!({
                    "codex": {
                        "quota_by_model": {
                            "gpt-5.6-sol": {"remaining_fraction": 0.75}
                        }
                    },
                    "codex_models": {"cards": {"old": {"slug": "old"}}}
                })),
                Some(1_740_000_000),
            )
            .await
            .expect("upstream metadata should update"));
        assert!(repository
            .update_key_model_fetch_success(
                "key-write-1",
                Some(&json!(["gpt-5.6-sol"])),
                1_740_000_002,
                &[ProviderCatalogUpstreamMetadataNamespaceUpdate {
                    namespace: "codex_models".to_string(),
                    value: json!({
                    "cards": {
                        "gpt-5.6-sol": {
                            "slug": "gpt-5.6-sol",
                            "use_responses_lite": true
                        }
                    }
                    }),
                }],
                Some(1_740_000_002),
            )
            .await
            .expect("model fetch success should update atomically"));
        assert!(repository
            .update_key_oauth_credentials(
                "key-write-1",
                "enc-key-2",
                Some("enc-auth-2"),
                Some(1_750_000_000),
            )
            .await
            .expect("oauth credentials should update"));
        assert!(repository
            .update_key_health_state(
                "key-write-1",
                true,
                Some(&json!({"openai:chat":{"score":0.9}})),
                None,
            )
            .await
            .expect("health state should update"));
        assert!(repository
            .clear_key_oauth_invalid_marker("key-write-1")
            .await
            .expect("oauth invalid marker should clear"));

        let reloaded_key = repository
            .list_keys_by_ids(&["key-write-1".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(
            reloaded_key.encrypted_api_key,
            Some("enc-key-2".to_string())
        );
        assert_eq!(
            reloaded_key.upstream_metadata,
            Some(json!({
                "codex": {
                    "quota_by_model": {
                        "gpt-5.6-sol": {"remaining_fraction": 0.75}
                    }
                },
                "codex_models": {
                    "cards": {
                        "gpt-5.6-sol": {
                            "slug": "gpt-5.6-sol",
                            "use_responses_lite": true
                        }
                    }
                }
            }))
        );
        assert_eq!(reloaded_key.allowed_models, Some(json!(["gpt-5.6-sol"])));
        assert_eq!(
            reloaded_key.last_models_fetch_at_unix_secs,
            Some(1_740_000_002)
        );
        assert!(reloaded_key.is_active);

        assert!(repository
            .delete_key("key-write-1")
            .await
            .expect("key should delete"));
        assert!(repository
            .delete_key("key-write-2")
            .await
            .expect("second key should delete"));
        assert!(repository
            .delete_endpoint("endpoint-write-1")
            .await
            .expect("endpoint should delete"));
        assert!(repository
            .delete_provider("provider-write-1")
            .await
            .expect("provider should delete"));
    }

    async fn seed_rows(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO providers (
  id, name, description, website, provider_type, provider_priority,
  monthly_quota_usd, monthly_used_usd,
  is_active, keep_priority_on_conversion, enable_format_conversion,
  config, created_at, updated_at
) VALUES (
  'provider-1', 'Provider One', 'test provider', 'https://example.com',
  'custom', 10, 0, 0, 1, 1, 1, '{"region":"us"}', 1, 2
)
"#,
        )
        .execute(pool)
        .await
        .expect("provider should seed");
        sqlx::query(
            r#"
INSERT INTO provider_endpoints (
  id, provider_id, name, base_url, api_format, api_family, endpoint_kind,
  is_active, health_score, header_rules, created_at, updated_at
) VALUES (
  'endpoint-1', 'provider-1', 'primary', 'https://api.example.com',
  'openai:chat', 'openai', 'chat', 1, 0.95, '{"Authorization":"Bearer"}', 3, 4
)
"#,
        )
        .execute(pool)
        .await
        .expect("endpoint should seed");
        sqlx::query(
            r#"
INSERT INTO provider_api_keys (
  id, provider_id, name, api_key, auth_type, capabilities, is_active,
  api_formats, auth_type_by_format, internal_priority, rpm_limit,
  concurrent_limit, request_count, total_tokens, total_cost_usd,
  success_count, error_count, total_response_time_ms, health_by_format,
  created_at, updated_at
) VALUES (
  'key-1', 'provider-1', 'default', 'enc-key', 'api_key',
  '{"cache_1h":true}', 1, '["openai:chat"]', '{"openai:chat":"api_key"}',
  5, 120, 3, 10, 1234, 1.5, 9, 1, 4294967296, '{"openai:chat":{"score":1}}',
  5, 6
)
"#,
        )
        .execute(pool)
        .await
        .expect("key should seed");
    }
}
