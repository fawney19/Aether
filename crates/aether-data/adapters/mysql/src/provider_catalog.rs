use std::collections::BTreeMap;

use async_trait::async_trait;
use sqlx::{
    mysql::{MySqlArguments, MySqlRow},
    query::Query,
    MySql, QueryBuilder, Row,
};

use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyAdaptiveStateUpdate, ProviderCatalogKeyAdminCasUpdate,
    ProviderCatalogKeyHealthStateUpdate, ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery,
    ProviderCatalogKeyOAuthCredentialCasDelete, ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ProviderCatalogKeyRuntimeMetadataUpdate, ProviderCatalogKeyStatusSnapshotUpdate,
    ProviderCatalogReadRepository, ProviderCatalogUpstreamMetadataNamespaceUpdate,
    ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
    StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::provider_key_scope::{
    plan_provider_key_scope_prune, serialize_provider_key_scope,
};
use aether_data_contracts::DataLayerError;
use aether_data_query::{
    push_ci_contains_any, push_eq, push_in, push_limit_offset, push_optional_eq, SqlDialect,
    WhereClause,
};

use crate::error::SqlResultExt;
use crate::MysqlPool;

#[derive(Debug, Clone)]
pub struct MysqlProviderCatalogReadRepository {
    pool: MysqlPool,
}

const PROVIDER_SELECT_SQL: &str = r#"
SELECT
  id, name, description, website, provider_type, billing_type,
  monthly_quota_usd, monthly_used_usd, quota_reset_day,
  quota_last_reset_at AS quota_last_reset_at_unix_secs,
  quota_expires_at AS quota_expires_at_unix_secs,
  provider_priority, is_active, keep_priority_on_conversion,
  enable_format_conversion, concurrent_limit, max_retries, proxy,
  request_timeout, stream_first_byte_timeout, config,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
FROM providers
"#;

const ENDPOINT_SELECT_SQL: &str = r#"
SELECT
  id, provider_id, api_format, api_family, endpoint_kind, is_active,
  health_score, base_url, header_rules, body_rules, max_retries,
  custom_path, config, format_acceptance_config, proxy,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
FROM provider_endpoints
"#;

const KEY_SELECT_SQL: &str = r#"
SELECT
  id, provider_id, name, auth_type, capabilities, is_active, api_formats,
  auth_type_by_format, allow_auth_channel_mismatch_formats,
  COALESCE(api_key, encrypted_key) AS api_key,
  auth_config, note, internal_priority, rate_multipliers,
  global_priority_by_format, allowed_models,
  expires_at AS expires_at_unix_secs,
  cache_ttl_minutes, max_probe_interval_minutes, proxy, fingerprint,
  rpm_limit, concurrent_limit, learned_rpm_limit, concurrent_429_count,
  rpm_429_count, last_429_at AS last_429_at_unix_secs, last_429_type,
  adjustment_history, utilization_samples,
  last_probe_increase_at AS last_probe_increase_at_unix_secs,
  last_rpm_peak, request_count, total_tokens, total_cost_usd,
  success_count, error_count, total_response_time_ms,
  last_used_at AS last_used_at_unix_secs, auto_fetch_models,
  last_models_fetch_at AS last_models_fetch_at_unix_secs,
  last_models_fetch_error, locked_models, model_include_patterns,
  model_exclude_patterns, upstream_metadata,
  oauth_invalid_at AS oauth_invalid_at_unix_secs,
  oauth_invalid_reason, status_snapshot,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs,
  health_by_format, circuit_breaker_by_format
FROM provider_api_keys
"#;

const KEY_SUMMARY_SELECT_SQL: &str = r#"
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
  CASE WHEN auth_config IS NULL THEN NULL ELSE '{}' END AS auth_config,
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
  CAST(0 AS SIGNED) AS total_tokens,
  CAST(0 AS DOUBLE) AS total_cost_usd,
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
"#;

const KEY_MAINTENANCE_SUMMARY_SELECT_SQL: &str = r#"
SELECT id, provider_id, is_active, upstream_metadata
FROM provider_api_keys
"#;

const KEY_STATS_SELECT_SQL: &str = r#"
SELECT
  provider_id,
  CAST(COUNT(*) AS SIGNED) AS total_keys,
  CAST(SUM(CASE WHEN is_active THEN 1 ELSE 0 END) AS SIGNED) AS active_keys
FROM provider_api_keys
"#;

impl MysqlProviderCatalogReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    pub async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = build_in_query(
            PROVIDER_SELECT_SQL,
            "id",
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
        let mut builder = QueryBuilder::<MySql>::new(PROVIDER_SELECT_SQL);
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

        let rows = build_in_query(
            ENDPOINT_SELECT_SQL,
            "id",
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

        let rows = build_in_query(
            ENDPOINT_SELECT_SQL,
            "provider_id",
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

        let rows = build_in_query(KEY_SELECT_SQL, "id", key_ids, " ORDER BY name ASC, id ASC")
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

        let rows = build_in_query(
            KEY_SELECT_SQL,
            "provider_id",
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

        let rows = build_in_query(
            KEY_SUMMARY_SELECT_SQL,
            "provider_id",
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

        let rows = build_in_query(
            KEY_MAINTENANCE_SUMMARY_SELECT_SQL,
            "provider_id",
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
        validate_non_empty(&query.provider_id, "provider catalog provider_id")?;
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

        let mut count_builder = build_key_page_count_query(query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?
            .max(0) as usize;

        let mut list_builder = build_key_page_list_query(query, limit, offset);
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

        let rows = build_in_query(
            KEY_STATS_SELECT_SQL,
            "provider_id",
            provider_ids,
            " GROUP BY provider_id ORDER BY provider_id ASC",
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
        if !key_ids.is_empty() {
            let removed = key_ids
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            Self::prune_provider_key_scope_references_in_tx(&mut tx, &removed).await?;
        }

        tx.commit().await.map_sql_err()?;
        Ok(())
    }

    /// Prunes `api_keys` and `user_groups` scope columns inside the caller's
    /// transaction. Rows are parsed with the unified compatible parser; a
    /// parse failure aborts the transaction instead of keeping a stale
    /// reference.
    async fn prune_provider_key_scope_references_in_tx(
        tx: &mut sqlx::Transaction<'_, MySql>,
        removed_key_ids: &std::collections::BTreeSet<String>,
    ) -> Result<u64, DataLayerError> {
        let mut updated = 0u64;
        for table in ["api_keys", "user_groups"] {
            let rows = sqlx::query(&format!(
                "SELECT id, allowed_provider_key_ids FROM {table} WHERE allowed_provider_key_ids IS NOT NULL"
            ))
            .fetch_all(&mut **tx)
            .await
            .map_sql_err()?;
            for row in rows {
                let id: String = row.try_get("id").map_sql_err()?;
                let Some(raw) = row
                    .try_get::<Option<String>, _>("allowed_provider_key_ids")
                    .map_sql_err()?
                else {
                    continue;
                };
                let field_name = format!("{table}.allowed_provider_key_ids");
                let plans = plan_provider_key_scope_prune(
                    std::iter::once((id.clone(), raw)),
                    removed_key_ids,
                    &field_name,
                )?;
                for plan in plans {
                    match plan.next {
                        None => {
                            sqlx::query(&format!(
                                "UPDATE {table} SET allowed_provider_key_ids = NULL WHERE id = ?"
                            ))
                            .bind(&plan.row_id)
                            .execute(&mut **tx)
                            .await
                            .map_sql_err()?;
                        }
                        Some(next) => {
                            let next_raw = serialize_provider_key_scope(Some(&next), &field_name)?;
                            sqlx::query(&format!(
                                "UPDATE {table} SET allowed_provider_key_ids = ? WHERE id = ?"
                            ))
                            .bind(&next_raw)
                            .bind(&plan.row_id)
                            .execute(&mut **tx)
                            .await
                            .map_sql_err()?;
                        }
                    }
                    updated += 1;
                }
            }
        }
        Ok(updated)
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

    pub async fn compare_and_update_key_admin_state(
        &self,
        update: &ProviderCatalogKeyAdminCasUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_admin_key_cas_update(update)?;
        let key = &update.key;
        let updated_at = key.updated_at_unix_secs.unwrap_or_else(current_unix_secs) as i64;
        let rotation_json = update
            .codex_rotation
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "provider_api_keys Codex rotation is not serializable: {err}"
                ))
            })?;
        let mut builder = QueryBuilder::<MySql>::new("UPDATE provider_api_keys SET provider_id = ");
        push_admin_key_assignments(&mut builder, key, updated_at)?;
        if update.reset_oauth_runtime {
            builder.push(", oauth_invalid_at = NULL, oauth_invalid_reason = NULL, error_count = 0");
        }
        if let Some(rotation_json) = rotation_json.as_deref() {
            builder
                .push(", upstream_metadata = JSON_SET(COALESCE(upstream_metadata, JSON_OBJECT()), '$.codex', CAST(")
                .push_bind(rotation_json)
                .push(" AS JSON))");
        }
        if update.codex_rotation.is_some() || update.reset_oauth_runtime {
            builder.push(", status_snapshot = ");
            match (update.codex_rotation.is_some(), update.reset_oauth_runtime) {
                (true, true) => builder.push("JSON_SET(COALESCE(status_snapshot, JSON_OBJECT()), '$.quota', CAST('null' AS JSON), '$.oauth', CAST('null' AS JSON))"),
                (true, false) => builder.push("JSON_SET(COALESCE(status_snapshot, JSON_OBJECT()), '$.quota', CAST('null' AS JSON))"),
                (false, true) => builder.push("JSON_SET(COALESCE(status_snapshot, JSON_OBJECT()), '$.oauth', CAST('null' AS JSON))"),
                (false, false) => unreachable!(),
            };
        }
        builder
            .push(" WHERE id = ")
            .push_bind(&key.id)
            .push(" AND BINARY api_key <=> BINARY ")
            .push_bind(update.expected_credential.encrypted_api_key.as_deref())
            .push(" AND BINARY auth_config <=> BINARY ")
            .push_bind(update.expected_encrypted_auth_config.as_deref())
            .push(" AND BINARY auth_type = BINARY ")
            .push_bind(&update.expected_credential.auth_type)
            .push(" AND BINARY provider_id = BINARY ")
            .push_bind(&update.expected_credential.provider_id)
            .push(
                " AND EXISTS (SELECT 1 FROM providers WHERE BINARY providers.id = BINARY provider_api_keys.provider_id AND BINARY providers.provider_type = BINARY ",
            )
            .push_bind(&update.expected_credential.provider_type)
            .push(")");
        if update.codex_rotation.is_some() {
            builder
                .push(" AND JSON_TYPE(COALESCE(upstream_metadata, JSON_OBJECT())) = 'OBJECT'")
                .push(" AND NOT (BINARY api_key <=> BINARY ")
                .push_bind(key.encrypted_api_key.as_deref())
                .push(" AND BINARY auth_config <=> BINARY ")
                .push_bind(key.encrypted_auth_config.as_deref())
                .push(" AND BINARY auth_type = BINARY ")
                .push_bind(&key.auth_type)
                .push(" AND BINARY provider_id = BINARY ")
                .push_bind(&key.provider_id)
                .push(")");
        }
        if update.codex_rotation.is_some() || update.reset_oauth_runtime {
            builder.push(" AND JSON_TYPE(COALESCE(status_snapshot, JSON_OBJECT())) = 'OBJECT'");
        }
        let rows_affected = builder
            .build()
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
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

    /// Deletes the key and prunes its id from every api_keys/user_groups key
    /// scope in one transaction. A scope parse failure or update error rolls
    /// the deletion back.
    pub async fn delete_provider_api_key_and_prune_scope(
        &self,
        key_id: &str,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(key_id, "provider catalog key_id")?;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let rows_affected = sqlx::query("DELETE FROM provider_api_keys WHERE id = ?")
            .bind(key_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?
            .rows_affected();
        if rows_affected == 0 {
            return Ok(false);
        }
        let removed =
            std::iter::once(key_id.to_string()).collect::<std::collections::BTreeSet<_>>();
        Self::prune_provider_key_scope_references_in_tx(&mut tx, &removed).await?;
        tx.commit().await.map_sql_err()?;
        Ok(true)
    }

    pub async fn compare_and_delete_key_oauth_credential(
        &self,
        delete: &ProviderCatalogKeyOAuthCredentialCasDelete,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(&delete.key_id, "provider catalog key_id")?;
        let expected = &delete.expected_credential;
        if expected
            .encrypted_api_key
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
            || expected.auth_type.trim().is_empty()
            || expected.provider_id.trim().is_empty()
            || expected.provider_type.trim().is_empty()
            || delete
                .expected_upstream_metadata_namespace
                .as_ref()
                .is_some_and(|expected| expected.namespace.trim().is_empty())
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog OAuth credential CAS delete contains empty fields".to_string(),
            ));
        }
        let mut builder = QueryBuilder::<MySql>::new("DELETE FROM provider_api_keys WHERE id = ");
        builder
            .push_bind(&delete.key_id)
            .push(" AND BINARY auth_config <=> BINARY ")
            .push_bind(delete.expected_encrypted_auth_config.as_deref())
            .push(" AND BINARY api_key <=> BINARY ")
            .push_bind(expected.encrypted_api_key.as_deref())
            .push(" AND BINARY auth_type = BINARY ")
            .push_bind(&expected.auth_type)
            .push(" AND BINARY provider_id = BINARY ")
            .push_bind(&expected.provider_id)
            .push(
                " AND EXISTS (SELECT 1 FROM providers WHERE \
                 BINARY providers.id = BINARY provider_api_keys.provider_id \
                 AND BINARY providers.provider_type = BINARY ",
            )
            .push_bind(&expected.provider_type)
            .push(")");
        if let Some(expected) = delete.expected_upstream_metadata_namespace.as_ref() {
            let namespace_path = format!(
                "$.{}",
                serde_json::to_string(&expected.namespace).map_err(|err| {
                    DataLayerError::UnexpectedValue(format!(
                        "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
                    ))
                })?
            );
            let expected_value = expected
                .expected_value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|err| {
                    DataLayerError::UnexpectedValue(format!(
                        "provider_api_keys.upstream_metadata expected value is not serializable: {err}"
                    ))
                })?;
            builder
                .push(" AND JSON_TYPE(COALESCE(NULLIF(upstream_metadata, ''), '{}')) = 'OBJECT'")
                .push(" AND JSON_EXTRACT(COALESCE(NULLIF(upstream_metadata, ''), '{}'), ")
                .push_bind(namespace_path)
                .push(") <=> CAST(")
                .push_bind(expected_value)
                .push(" AS JSON)");
        }
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let rows_affected = builder
            .build()
            .execute(&mut *tx)
            .await
            .map_sql_err()?
            .rows_affected();
        if rows_affected == 0 {
            return Ok(false);
        }
        // The CAS condition matched: prune the key id from every
        // api_keys/user_groups key scope inside the same transaction so no
        // policy ever references a deleted provider key.
        let removed =
            std::iter::once(delete.key_id.clone()).collect::<std::collections::BTreeSet<_>>();
        Self::prune_provider_key_scope_references_in_tx(&mut tx, &removed).await?;
        tx.commit().await.map_sql_err()?;
        Ok(true)
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
SET upstream_metadata = JSON_SET(
      COALESCE(NULLIF(upstream_metadata, ''), '{}'),
      ?, CAST(? AS JSON)
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
SET upstream_metadata = JSON_SET(
      COALESCE(NULLIF(upstream_metadata, ''), '{}'),
      ?, CAST(? AS JSON)
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

    pub async fn compare_and_update_key_oauth_runtime_state(
        &self,
        update: &ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_non_empty(&update.key_id, "provider catalog key_id")?;
        validate_non_empty(
            &update.encrypted_auth_config,
            "provider catalog OAuth auth_config",
        )?;
        if update
            .encrypted_api_key_update
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog OAuth api_key update must not be empty".to_string(),
            ));
        }
        if update.expected_credential.as_ref().is_some_and(|expected| {
            expected
                .encrypted_api_key
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
                || expected.auth_type.trim().is_empty()
                || expected.provider_id.trim().is_empty()
                || expected.provider_type.trim().is_empty()
        }) {
            return Err(DataLayerError::InvalidInput(
                "provider catalog OAuth credential fence must not contain empty fields".to_string(),
            ));
        }
        if update
            .expected_upstream_metadata_namespace
            .as_ref()
            .is_some_and(|expected| expected.namespace.trim().is_empty())
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog OAuth runtime metadata namespace must not be empty".to_string(),
            ));
        }
        if update
            .upstream_metadata_namespace_to_remove
            .as_deref()
            .is_some_and(|namespace| namespace.trim().is_empty())
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog OAuth runtime metadata namespace to remove must not be empty"
                    .to_string(),
            ));
        }
        if !update.status_snapshot_patch.is_object() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog status snapshot patch must be an object".to_string(),
            ));
        }
        if update
            .upstream_metadata_patch
            .as_ref()
            .is_some_and(|patch| !patch.is_object())
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog upstream metadata patch must be an object".to_string(),
            ));
        }
        if update
            .upstream_metadata_namespace_to_remove
            .as_ref()
            .is_some_and(|namespace| {
                update
                    .upstream_metadata_patch
                    .as_ref()
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|patch| patch.contains_key(namespace))
            })
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog OAuth runtime metadata namespace cannot be patched and removed in the same update"
                    .to_string(),
            ));
        }
        let mut builder =
            QueryBuilder::<MySql>::new("UPDATE provider_api_keys SET oauth_invalid_at = ");
        builder
            .push_bind(optional_i64_from_u64(
                update.oauth_invalid_at_unix_secs,
                "provider_api_keys.oauth_invalid_at",
            )?)
            .push(", oauth_invalid_reason = ")
            .push_bind(update.oauth_invalid_reason.as_deref())
            .push(", auth_config = ")
            .push_bind(&update.encrypted_auth_config);
        if let Some(encrypted_api_key) = update.encrypted_api_key_update.as_deref() {
            builder.push(", api_key = ").push_bind(encrypted_api_key);
        }
        if let Some(expires_at_unix_secs) = update.expires_at_unix_secs_update {
            builder
                .push(", expires_at = ")
                .push_bind(optional_i64_from_u64(
                    expires_at_unix_secs,
                    "provider_api_keys.expires_at",
                )?);
        }
        if update.upstream_metadata_patch.is_some()
            || update.upstream_metadata_namespace_to_remove.is_some()
        {
            builder.push(", upstream_metadata = ");
            if update.upstream_metadata_namespace_to_remove.is_some() {
                builder.push("JSON_REMOVE(");
            }
            if let Some(metadata_patch) = update.upstream_metadata_patch.as_ref() {
                push_upstream_metadata_shallow_patch(&mut builder, metadata_patch)?;
            } else {
                builder.push("COALESCE(NULLIF(upstream_metadata, ''), '{}')");
            }
            if let Some(namespace) = update.upstream_metadata_namespace_to_remove.as_ref() {
                let namespace_path = format!(
                    "$.{}",
                    serde_json::to_string(namespace).map_err(|err| {
                        DataLayerError::UnexpectedValue(format!(
                            "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
                        ))
                    })?
                );
                builder.push(", ").push_bind(namespace_path).push(")");
            }
        }
        builder.push(", status_snapshot = ");
        push_status_snapshot_shallow_patch(&mut builder, &update.status_snapshot_patch)?;
        if update.reset_error_count {
            builder.push(", error_count = 0");
        }
        builder
            .push(", updated_at = ")
            .push_bind(
                update
                    .updated_at_unix_secs
                    .unwrap_or_else(current_unix_secs) as i64,
            )
            .push(" WHERE id = ")
            .push_bind(&update.key_id)
            .push(" AND BINARY auth_config <=> BINARY ")
            .push_bind(update.expected_encrypted_auth_config.as_deref());
        if let Some(expected) = update.expected_credential.as_ref() {
            builder
                .push(" AND BINARY api_key <=> BINARY ")
                .push_bind(expected.encrypted_api_key.as_deref())
                .push(" AND BINARY auth_type = BINARY ")
                .push_bind(&expected.auth_type)
                .push(" AND BINARY provider_id = BINARY ")
                .push_bind(&expected.provider_id)
                .push(
                    " AND EXISTS (SELECT 1 FROM providers WHERE \
                     BINARY providers.id = BINARY provider_api_keys.provider_id \
                     AND BINARY providers.provider_type = BINARY ",
                )
                .push_bind(&expected.provider_type)
                .push(")");
        }
        if update.expected_upstream_metadata_namespace.is_some()
            || update.upstream_metadata_patch.is_some()
            || update.upstream_metadata_namespace_to_remove.is_some()
        {
            builder
                .push(" AND JSON_TYPE(COALESCE(NULLIF(upstream_metadata, ''), '{}')) = 'OBJECT'");
        }
        if let Some(expected) = update.expected_upstream_metadata_namespace.as_ref() {
            let namespace_path = format!(
                "$.{}",
                serde_json::to_string(&expected.namespace).map_err(|err| {
                    DataLayerError::UnexpectedValue(format!(
                        "provider_api_keys.upstream_metadata namespace is not serializable: {err}"
                    ))
                })?
            );
            let expected_value = expected
                .expected_value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|err| {
                    DataLayerError::UnexpectedValue(format!(
                        "provider_api_keys.upstream_metadata expected value is not serializable: {err}"
                    ))
                })?;
            builder
                .push(" AND JSON_EXTRACT(COALESCE(NULLIF(upstream_metadata, ''), '{}'), ")
                .push_bind(namespace_path)
                .push(") <=> CAST(")
                .push_bind(expected_value)
                .push(" AS JSON)");
        }
        let rows_affected = builder
            .build()
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
            QueryBuilder::<MySql>::new("UPDATE provider_api_keys SET learned_rpm_limit = ");
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
            .push(" AND learned_rpm_limit <=> ")
            .push_bind(optional_i64_from_u32(expected.learned_rpm_limit))
            .push(" AND rpm_429_count <=> ")
            .push_bind(optional_i64_from_u32(expected.rpm_429_count))
            .push(" AND last_429_at <=> ")
            .push_bind(optional_i64_from_u64(
                expected.last_429_at_unix_secs,
                "provider_api_keys.last_429_at",
            )?)
            .push(" AND last_429_type <=> ")
            .push_bind(&expected.last_429_type)
            .push(" AND JSON_EXTRACT(adjustment_history, '$') <=> CAST(")
            .push_bind(optional_json_to_string(
                &expected.adjustment_history,
                "provider_api_keys.adjustment_history",
            )?)
            .push(" AS JSON)")
            .push(" AND JSON_EXTRACT(utilization_samples, '$') <=> CAST(")
            .push_bind(optional_json_to_string(
                &expected.utilization_samples,
                "provider_api_keys.utilization_samples",
            )?)
            .push(" AS JSON)")
            .push(" AND last_probe_increase_at <=> ")
            .push_bind(optional_i64_from_u64(
                expected.last_probe_increase_at_unix_secs,
                "provider_api_keys.last_probe_increase_at",
            )?)
            .push(" AND last_rpm_peak <=> ")
            .push_bind(optional_i64_from_u32(expected.last_rpm_peak))
            .push(" AND concurrent_429_count <=> ")
            .push_bind(optional_i64_from_u32(expected.concurrent_429_count));
        if let Some(expected_encrypted_auth_config) =
            update.expected_encrypted_auth_config.as_deref()
        {
            builder
                .push(" AND auth_config <=> ")
                .push_bind(expected_encrypted_auth_config);
        }
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
        let mut builder = QueryBuilder::<MySql>::new(
            "UPDATE provider_api_keys SET upstream_metadata = JSON_SET(\
             COALESCE(NULLIF(upstream_metadata, ''), '{}'), ",
        );
        builder
            .push_bind(namespace_path.clone())
            .push(", CAST(")
            .push_bind(metadata_value)
            .push(" AS JSON)), status_snapshot = ");
        push_status_snapshot_shallow_patch(&mut builder, &update.status_snapshot_patch)?;
        builder
            .push(", updated_at = ")
            .push_bind(
                update
                    .updated_at_unix_secs
                    .unwrap_or_else(current_unix_secs) as i64,
            )
            .push(" WHERE id = ")
            .push_bind(&update.key_id)
            .push(" AND JSON_TYPE(COALESCE(NULLIF(upstream_metadata, ''), '{}')) = 'OBJECT'")
            .push(" AND JSON_EXTRACT(COALESCE(NULLIF(upstream_metadata, ''), '{}'), ")
            .push_bind(namespace_path)
            .push(") <=> CAST(")
            .push_bind(expected_metadata_value)
            .push(" AS JSON)");
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
            QueryBuilder::<MySql>::new("UPDATE provider_api_keys SET status_snapshot = ");
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
  AND JSON_EXTRACT(health_by_format, '$') <=> CAST(? AS JSON)
  AND JSON_EXTRACT(circuit_breaker_by_format, '$') <=> CAST(? AS JSON)
  AND (? IS NULL OR auth_config <=> ?)
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
        .bind(update.expected_encrypted_auth_config.as_deref())
        .bind(update.expected_encrypted_auth_config.as_deref())
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
impl ProviderCatalogReadRepository for MysqlProviderCatalogReadRepository {
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
impl ProviderCatalogWriteRepository for MysqlProviderCatalogReadRepository {
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

    async fn compare_and_update_key_admin_state(
        &self,
        update: &ProviderCatalogKeyAdminCasUpdate,
    ) -> Result<bool, DataLayerError> {
        Self::compare_and_update_key_admin_state(self, update).await
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

    async fn compare_and_delete_key_oauth_credential(
        &self,
        delete: &ProviderCatalogKeyOAuthCredentialCasDelete,
    ) -> Result<bool, DataLayerError> {
        Self::compare_and_delete_key_oauth_credential(self, delete).await
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

    async fn compare_and_update_key_oauth_runtime_state(
        &self,
        update: &ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ) -> Result<bool, DataLayerError> {
        Self::compare_and_update_key_oauth_runtime_state(self, update).await
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
    builder: &mut QueryBuilder<'args, MySql>,
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

    builder.push("JSON_SET(COALESCE(NULLIF(status_snapshot, ''), '{}')");
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
        builder.push(", ").push_bind(path).push(", CAST(");
        builder.push_bind(value).push(" AS JSON)");
    }
    builder.push(")");
    Ok(())
}

fn push_upstream_metadata_shallow_patch<'args>(
    builder: &mut QueryBuilder<'args, MySql>,
    patch: &serde_json::Value,
) -> Result<(), DataLayerError> {
    let object = patch.as_object().ok_or_else(|| {
        DataLayerError::InvalidInput(
            "provider catalog upstream metadata patch must be an object".to_string(),
        )
    })?;
    if object.is_empty() {
        builder.push("COALESCE(NULLIF(upstream_metadata, ''), '{}')");
        return Ok(());
    }

    builder.push("JSON_SET(COALESCE(NULLIF(upstream_metadata, ''), '{}')");
    for (field, value) in object {
        let path = format!(
            "$.{}",
            serde_json::to_string(field).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "provider_api_keys.upstream_metadata field is not serializable: {err}"
                ))
            })?
        );
        let value = serde_json::to_string(value).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "provider_api_keys.upstream_metadata value is not serializable: {err}"
            ))
        })?;
        builder.push(", ").push_bind(path).push(", CAST(");
        builder.push_bind(value).push(" AS JSON)");
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

fn build_in_query<'a>(
    select_sql: &'static str,
    column: &'static str,
    values: &'a [String],
    suffix: &'static str,
) -> QueryBuilder<'a, MySql> {
    let mut builder = QueryBuilder::<MySql>::new(select_sql);
    let mut where_clause = WhereClause::new();
    push_in(&mut builder, &mut where_clause, column, values);
    builder.push(suffix);
    builder
}

fn build_key_page_count_query<'a>(
    query: &'a ProviderCatalogKeyListQuery,
) -> QueryBuilder<'a, MySql> {
    let mut builder = QueryBuilder::<MySql>::new(
        "SELECT CAST(COUNT(*) AS SIGNED) AS total FROM provider_api_keys",
    );
    let mut where_clause = WhereClause::new();
    apply_key_page_filters(&mut builder, &mut where_clause, query);
    builder
}

fn build_key_page_list_query<'a>(
    query: &'a ProviderCatalogKeyListQuery,
    limit: i64,
    offset: i64,
) -> QueryBuilder<'a, MySql> {
    let mut builder = QueryBuilder::<MySql>::new(KEY_SELECT_SQL);
    let mut where_clause = WhereClause::new();
    apply_key_page_filters(&mut builder, &mut where_clause, query);
    builder
        .push(" ORDER BY ")
        .push(key_page_order_by(&query.order));
    push_limit_offset(&mut builder, limit, offset);
    builder
}

fn key_page_order_by(order: &ProviderCatalogKeyListOrder) -> &'static str {
    match order {
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
    }
}

fn apply_key_page_filters<'a>(
    builder: &mut QueryBuilder<'a, MySql>,
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
            SqlDialect::MySql,
            &["name", "id"],
            search,
        );
    }
    push_optional_eq(builder, where_clause, "is_active", query.is_active);
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
  AND BINARY provider_id = BINARY ?
  AND BINARY auth_type = BINARY ?
  AND BINARY api_key <=> BINARY ?
  AND BINARY auth_config <=> BINARY ?
"#
}

fn key_update_query(
    key: &StoredProviderCatalogKey,
    updated_at: i64,
) -> Result<Query<'_, MySql, MySqlArguments>, DataLayerError> {
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
        .bind(&key.id)
        .bind(&key.provider_id)
        .bind(&key.auth_type)
        .bind(&key.encrypted_api_key)
        .bind(&key.encrypted_auth_config))
}

fn push_admin_key_assignments<'args>(
    builder: &mut QueryBuilder<'args, MySql>,
    key: &'args StoredProviderCatalogKey,
    updated_at: i64,
) -> Result<(), DataLayerError> {
    builder
        .push_bind(&key.provider_id)
        .push(", name = ")
        .push_bind(&key.name)
        .push(", api_key = ")
        .push_bind(&key.encrypted_api_key)
        .push(", auth_type = ")
        .push_bind(&key.auth_type)
        .push(", capabilities = ")
        .push_bind(optional_json_to_string(
            &key.capabilities,
            "provider_api_keys.capabilities",
        )?)
        .push(", is_active = ")
        .push_bind(key.is_active)
        .push(", api_formats = ")
        .push_bind(optional_json_to_string(
            &key.api_formats,
            "provider_api_keys.api_formats",
        )?)
        .push(", auth_type_by_format = ")
        .push_bind(optional_json_to_string(
            &key.auth_type_by_format,
            "provider_api_keys.auth_type_by_format",
        )?)
        .push(", allow_auth_channel_mismatch_formats = ")
        .push_bind(optional_json_to_string(
            &key.allow_auth_channel_mismatch_formats,
            "provider_api_keys.allow_auth_channel_mismatch_formats",
        )?)
        .push(", auth_config = ")
        .push_bind(&key.encrypted_auth_config)
        .push(", note = ")
        .push_bind(&key.note)
        .push(", internal_priority = ")
        .push_bind(key.internal_priority)
        .push(", rate_multipliers = ")
        .push_bind(optional_json_to_string(
            &key.rate_multipliers,
            "provider_api_keys.rate_multipliers",
        )?)
        .push(", global_priority_by_format = ")
        .push_bind(optional_json_to_string(
            &key.global_priority_by_format,
            "provider_api_keys.global_priority_by_format",
        )?)
        .push(", allowed_models = ")
        .push_bind(optional_json_to_string(
            &key.allowed_models,
            "provider_api_keys.allowed_models",
        )?)
        .push(", expires_at = ")
        .push_bind(optional_i64_from_u64(
            key.expires_at_unix_secs,
            "provider_api_keys.expires_at",
        )?)
        .push(", cache_ttl_minutes = ")
        .push_bind(key.cache_ttl_minutes)
        .push(", max_probe_interval_minutes = ")
        .push_bind(key.max_probe_interval_minutes)
        .push(", proxy = ")
        .push_bind(optional_json_to_string(
            &key.proxy,
            "provider_api_keys.proxy",
        )?)
        .push(", fingerprint = ")
        .push_bind(optional_json_to_string(
            &key.fingerprint,
            "provider_api_keys.fingerprint",
        )?)
        .push(", rpm_limit = ")
        .push_bind(optional_i64_from_u32(key.rpm_limit))
        .push(", concurrent_limit = ")
        .push_bind(key.concurrent_limit)
        .push(", auto_fetch_models = ")
        .push_bind(key.auto_fetch_models)
        .push(", locked_models = ")
        .push_bind(optional_json_to_string(
            &key.locked_models,
            "provider_api_keys.locked_models",
        )?)
        .push(", model_include_patterns = ")
        .push_bind(optional_json_to_string(
            &key.model_include_patterns,
            "provider_api_keys.model_include_patterns",
        )?)
        .push(", model_exclude_patterns = ")
        .push_bind(optional_json_to_string(
            &key.model_exclude_patterns,
            "provider_api_keys.model_exclude_patterns",
        )?)
        .push(", updated_at = ")
        .push_bind(updated_at);
    Ok(())
}

fn validate_admin_key_cas_update(
    update: &ProviderCatalogKeyAdminCasUpdate,
) -> Result<(), DataLayerError> {
    validate_key(&update.key)?;
    let expected = &update.expected_credential;
    if expected.auth_type.trim().is_empty()
        || expected.provider_id.trim().is_empty()
        || expected.provider_type.trim().is_empty()
        || expected
            .encrypted_api_key
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || update
            .expected_encrypted_auth_config
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err(DataLayerError::InvalidInput(
            "provider catalog admin credential fence contains empty fields".to_string(),
        ));
    }
    let Some(rotation) = update.codex_rotation.as_ref() else {
        return Ok(());
    };
    let valid_rotation = expected.provider_type.eq_ignore_ascii_case("codex")
        && rotation.as_object().is_some_and(|object| {
            object.len() == 1
                && object
                    .get("credential_generation")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|generation| !generation.trim().is_empty())
        });
    if !valid_rotation {
        return Err(DataLayerError::InvalidInput(
            "provider catalog Codex rotation must contain only credential_generation".to_string(),
        ));
    }
    Ok(())
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

fn map_provider_row(row: &MySqlRow) -> Result<StoredProviderCatalogProvider, DataLayerError> {
    Ok(StoredProviderCatalogProvider::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("website").map_sql_err()?,
        row.try_get("provider_type").map_sql_err()?,
    )?
    .with_description(row.try_get("description").map_sql_err()?)
    .with_billing_fields(
        row.try_get("billing_type").map_sql_err()?,
        row.try_get("monthly_quota_usd").map_sql_err()?,
        row.try_get("monthly_used_usd").map_sql_err()?,
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

fn map_endpoint_row(row: &MySqlRow) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
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
    .with_health_score(
        row.try_get::<Option<f64>, _>("health_score")
            .map_sql_err()?
            .unwrap_or(1.0),
    )
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

fn map_key_stats_row(row: &MySqlRow) -> Result<StoredProviderCatalogKeyStats, DataLayerError> {
    StoredProviderCatalogKeyStats::new(
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("total_keys").map_sql_err()?,
        row.try_get("active_keys").map_sql_err()?,
    )
}

fn map_key_maintenance_summary_row(
    row: &MySqlRow,
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

fn map_key_row(row: &MySqlRow) -> Result<StoredProviderCatalogKey, DataLayerError> {
    let total_cost_usd = row
        .try_get::<Option<f64>, _>("total_cost_usd")
        .map_sql_err()?
        .unwrap_or(0.0);
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
        key.internal_priority = row.try_get("internal_priority").unwrap_or(50);
        key.cache_ttl_minutes = row.try_get("cache_ttl_minutes").unwrap_or(5);
        key.max_probe_interval_minutes = row.try_get("max_probe_interval_minutes").unwrap_or(32);
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
    use super::MysqlProviderCatalogReadRepository;
    use crate::run_migrations;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery, StoredProviderCatalogEndpoint,
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;
    use sqlx::Execute;

    #[test]
    fn ordinary_key_update_does_not_own_adaptive_runtime_fields() {
        let sql = super::key_update_sql().to_ascii_lowercase();
        for runtime_assignment in [
            "learned_rpm_limit =",
            "rpm_429_count =",
            "last_429_at =",
            "last_429_type =",
            "adjustment_history =",
            "utilization_samples =",
            "last_probe_increase_at =",
            "last_rpm_peak =",
        ] {
            assert!(!sql.contains(runtime_assignment));
        }
        assert!(sql.contains("binary api_key <=> binary ?"));
        assert!(sql.contains("binary auth_config <=> binary ?"));
    }

    #[test]
    fn admin_credential_cas_has_atomic_rotation_guards() {
        let source = include_str!("provider_catalog.rs");
        for predicate in [
            "BINARY api_key <=> BINARY ",
            "BINARY auth_config <=> BINARY ",
            "JSON_TYPE(COALESCE(upstream_metadata, JSON_OBJECT())) = 'OBJECT'",
            "JSON_TYPE(COALESCE(status_snapshot, JSON_OBJECT())) = 'OBJECT'",
            "JSON_SET(COALESCE(upstream_metadata, JSON_OBJECT()), '$.codex'",
            "oauth_invalid_at = NULL, oauth_invalid_reason = NULL, error_count = 0",
        ] {
            assert!(
                source.contains(predicate),
                "missing admin CAS guard: {predicate}"
            );
        }
    }

    #[test]
    fn runtime_metadata_cas_requires_an_object_metadata_root() {
        let source = include_str!("provider_catalog.rs");
        assert!(
            source.contains("JSON_TYPE(COALESCE(NULLIF(upstream_metadata, ''), '{}')) = 'OBJECT'")
        );
    }
    #[tokio::test]
    async fn empty_id_lists_do_not_connect_to_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let repository = MysqlProviderCatalogReadRepository::new(pool);
        assert!(repository
            .list_providers_by_ids(&[])
            .await
            .expect("empty provider ids should not connect")
            .is_empty());
        assert!(repository
            .list_endpoints_by_ids(&[])
            .await
            .expect("empty endpoint ids should not connect")
            .is_empty());
        assert!(repository
            .list_endpoints_by_provider_ids(&[])
            .await
            .expect("empty endpoint provider ids should not connect")
            .is_empty());
        assert!(repository
            .list_keys_by_ids(&[])
            .await
            .expect("empty key ids should not connect")
            .is_empty());
        assert!(repository
            .list_keys_by_provider_ids(&[])
            .await
            .expect("empty key provider ids should not connect")
            .is_empty());
        assert!(repository
            .list_key_summaries_by_provider_ids(&[])
            .await
            .expect("empty key summary provider ids should not connect")
            .is_empty());
        assert!(repository
            .list_key_maintenance_summaries_by_provider_ids(&[])
            .await
            .expect("empty maintenance provider ids should not connect")
            .is_empty());
        assert!(repository
            .list_key_stats_by_provider_ids(&[])
            .await
            .expect("empty stats provider ids should not connect")
            .is_empty());
    }

    #[test]
    fn list_query_targets_ids_and_index_aligned_ordering() {
        let provider_ids = vec!["provider-a".to_string(), "provider-b".to_string()];
        let mut builder = super::build_in_query(
            super::KEY_SELECT_SQL,
            "provider_id",
            &provider_ids,
            " ORDER BY provider_id ASC, name ASC, id ASC",
        );
        let query = builder.build();
        let sql = query.sql();

        assert!(sql.contains("WHERE provider_id IN ("));
        assert!(sql.contains("ORDER BY provider_id ASC, name ASC, id ASC"));
    }

    #[test]
    fn key_page_builders_apply_matching_filters_and_requested_order() {
        let query = ProviderCatalogKeyListQuery {
            provider_id: "provider-a".to_string(),
            search: Some("Primary".to_string()),
            is_active: Some(true),
            offset: 20,
            limit: 10,
            order: ProviderCatalogKeyListOrder::LastUsedAtDesc,
        };
        let mut count_builder = super::build_key_page_count_query(&query);
        let count_sql = count_builder.build().sql().to_string();
        let mut list_builder = super::build_key_page_list_query(&query, 10, 20);
        let list_sql = list_builder.build().sql().to_string();

        for sql in [&count_sql, &list_sql] {
            assert!(sql.contains("provider_id = ?"));
            assert!(sql.contains("LOWER(name) LIKE ?"));
            assert!(sql.contains("LOWER(id) LIKE ?"));
            assert!(sql.contains("is_active = ?"));
        }
        assert!(list_sql
            .contains("ORDER BY last_used_at IS NULL ASC, last_used_at DESC, name ASC, id ASC"));
        assert!(list_sql.contains("LIMIT ? OFFSET ?"));
    }

    #[test]
    fn key_page_orders_match_cross_driver_contract() {
        for (order, expected) in [
            (
                ProviderCatalogKeyListOrder::Name,
                "internal_priority ASC, name ASC, id ASC",
            ),
            (
                ProviderCatalogKeyListOrder::CreatedAt,
                "internal_priority ASC, COALESCE(created_at, 0) ASC, id ASC",
            ),
            (
                ProviderCatalogKeyListOrder::CreatedAtAsc,
                "created_at IS NULL ASC, created_at ASC, name ASC, id ASC",
            ),
            (
                ProviderCatalogKeyListOrder::CreatedAtDesc,
                "created_at IS NULL ASC, created_at DESC, name ASC, id ASC",
            ),
            (
                ProviderCatalogKeyListOrder::LastUsedAtAsc,
                "last_used_at IS NULL ASC, last_used_at ASC, name ASC, id ASC",
            ),
            (
                ProviderCatalogKeyListOrder::LastUsedAtDesc,
                "last_used_at IS NULL ASC, last_used_at DESC, name ASC, id ASC",
            ),
        ] {
            assert_eq!(super::key_page_order_by(&order), expected);
        }
    }

    #[tokio::test]
    async fn mysql_provider_catalog_repository_round_trips_when_url_is_set() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!(
                "skipping mysql provider catalog smoke test because AETHER_TEST_MYSQL_URL is unset"
            );
            return;
        };

        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("mysql test pool should connect");
        run_migrations(&pool)
            .await
            .expect("mysql migrations should run");

        let suffix = unique_suffix();
        let provider_id = format!("provider-{suffix}");
        let endpoint_id = format!("endpoint-{suffix}");
        let key_id = format!("key-{suffix}");
        let repository = MysqlProviderCatalogReadRepository::new(pool);

        let provider = StoredProviderCatalogProvider::new(
            provider_id.clone(),
            format!("Provider {suffix}"),
            Some(format!("https://{suffix}.example.com")),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_description(Some("mysql smoke provider".to_string()))
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

        let endpoint = StoredProviderCatalogEndpoint::new(
            endpoint_id.clone(),
            provider_id.clone(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_health_score(0.88)
        .with_transport_fields(
            "https://api.example.com/v1".to_string(),
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

        let key = StoredProviderCatalogKey::new(
            key_id.clone(),
            provider_id.clone(),
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
        .with_usage_fields(Some(1), Some(250))
        .with_usage_totals(1234, 1.5)
        .with_health_fields(
            Some(json!({"openai:chat":{"score":1}})),
            Some(json!({"openai:chat":{"open":false}})),
        );
        let created_key = repository
            .create_key(&key)
            .await
            .expect("key should create");
        assert_eq!(created_key.concurrent_limit, Some(3));

        let providers = repository
            .list_providers_by_ids(std::slice::from_ref(&provider_id))
            .await
            .expect("providers should list by ids");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, provider_id);
        assert!(repository
            .list_providers(true)
            .await
            .expect("active providers should list")
            .iter()
            .any(|provider| provider.id == provider_id));

        let endpoints = repository
            .list_endpoints_by_ids(std::slice::from_ref(&endpoint_id))
            .await
            .expect("endpoints should list by ids");
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].id, endpoint_id);
        assert_eq!(
            repository
                .list_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
                .await
                .expect("endpoints should list by provider")
                .len(),
            1
        );

        let keys = repository
            .list_keys_by_provider_ids(std::slice::from_ref(&provider_id))
            .await
            .expect("keys should list");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].total_tokens, 1234);

        let summaries = repository
            .list_key_summaries_by_provider_ids(std::slice::from_ref(&provider_id))
            .await
            .expect("key summaries should list");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].encrypted_api_key.as_deref(), Some("summary"));
        assert_eq!(summaries[0].total_tokens, 0);

        let page = repository
            .list_keys_page(&ProviderCatalogKeyListQuery {
                provider_id: provider_id.clone(),
                search: Some("default".to_string()),
                is_active: Some(true),
                offset: 0,
                limit: 10,
                order: ProviderCatalogKeyListOrder::Name,
            })
            .await
            .expect("filtered key page should list");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, key_id);

        let stats = repository
            .list_key_stats_by_provider_ids(std::slice::from_ref(&provider_id))
            .await
            .expect("key stats should list");
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].total_keys, 1);
        assert_eq!(stats[0].active_keys, 1);

        assert!(repository
            .update_key_upstream_metadata(
                &key_id,
                Some(&json!({"models":["gpt-4.1-mini"]})),
                Some(1_740_000_000),
            )
            .await
            .expect("upstream metadata should update"));
        assert!(repository
            .update_key_health_state(
                &key_id,
                false,
                Some(&json!({"openai:chat":{"score":0.9}})),
                None,
            )
            .await
            .expect("health state should update"));

        let reloaded_key = repository
            .list_keys_by_ids(std::slice::from_ref(&key_id))
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(
            reloaded_key.upstream_metadata,
            Some(json!({"models":["gpt-4.1-mini"]}))
        );
        assert!(!reloaded_key.is_active);

        let maintenance = repository
            .list_key_maintenance_summaries_by_provider_ids(std::slice::from_ref(&provider_id))
            .await
            .expect("maintenance summaries should list");
        assert_eq!(maintenance.len(), 1);
        assert!(!maintenance[0].is_active);
        assert_eq!(
            maintenance[0].upstream_metadata,
            Some(json!({"models":["gpt-4.1-mini"]}))
        );

        assert!(repository
            .delete_key(&key_id)
            .await
            .expect("key should delete"));
        assert!(repository
            .delete_endpoint(&endpoint_id)
            .await
            .expect("endpoint should delete"));
        assert!(repository
            .delete_provider(&provider_id)
            .await
            .expect("provider should delete"));
    }

    fn unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }

    /// Fault-injection / rollback coverage for the transactional key-delete +
    /// scope-prune path. Requires a live MySQL at AETHER_TEST_MYSQL_URL;
    /// skipped when the variable is unset (CI without a database).
    #[tokio::test]
    async fn mysql_delete_key_prune_scope_is_atomic_and_reports_parse_failures() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL").ok() else {
            eprintln!("AETHER_TEST_MYSQL_URL unset; skipping mysql fault-injection test");
            return;
        };
        // Use a dedicated database so this test never collides with the
        // repository's other AETHER_TEST_MYSQL_URL tests. The URL must point
        // at a MySQL server; the database name is derived and created here.
        let server_url = database_url
            .rsplit_once('/')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_else(|| database_url.clone());
        let scope_url = format!("{server_url}/aether_test_scope");
        let admin_pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .expect("mysql admin pool should connect");
        sqlx::query("DROP DATABASE IF EXISTS aether_test_scope")
            .execute(&admin_pool)
            .await
            .expect("database should drop");
        sqlx::query("CREATE DATABASE aether_test_scope")
            .execute(&admin_pool)
            .await
            .expect("database should create");
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&scope_url)
            .await
            .expect("mysql pool should connect");
        crate::run_migrations(&pool)
            .await
            .expect("migrations should run");

        sqlx::query(
            "INSERT INTO providers (id, name, provider_type, is_active, provider_priority, created_at, updated_at) VALUES ('provider-1', 'P1', 'custom', 1, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .expect("provider should seed");
        sqlx::query(
            "INSERT INTO provider_api_keys (id, provider_id, name, api_key, auth_type, is_active, internal_priority, created_at, updated_at) VALUES ('key-1', 'provider-1', 'one', 'enc', 'api_key', 1, 1, 1, 1), ('key-2', 'provider-1', 'two', 'enc', 'api_key', 1, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .expect("keys should seed");
        sqlx::query(
            r#"INSERT INTO api_keys (id, user_id, key_hash, allowed_providers, allowed_provider_key_ids, rate_limit, is_active, is_standalone, total_requests, total_tokens, total_cost_usd, created_at, updated_at) VALUES ('auth-key-1', 'user-1', 'hash-1', '["provider-1"]', '{"provider-1":["key-1"]}', 100, 1, 1, 0, 0, 0, 1, 1)"#,
        )
        .execute(&pool)
        .await
        .expect("api key should seed");
        sqlx::query(
            r#"INSERT INTO user_groups (id, name, normalized_name, priority, allowed_providers, allowed_provider_key_ids, allowed_providers_mode, allowed_api_formats_mode, allowed_models_mode, rate_limit_mode, created_at, updated_at) VALUES ('group-1', 'G', 'g', 0, '["provider-1"]', '{"provider-1":["key-1"]}', 'specific', 'inherit', 'inherit', 'inherit', 1, 1)"#,
        )
        .execute(&pool)
        .await
        .expect("group should seed");

        let repository = MysqlProviderCatalogReadRepository::new(pool.clone());
        let deleted = repository
            .delete_provider_api_key_and_prune_scope("key-1")
            .await
            .expect("delete should succeed");
        assert!(deleted);
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_api_keys WHERE id = 'key-1'")
                .fetch_one(&pool)
                .await
                .expect("count should load");
        assert_eq!(remaining, 0);
        let scope: Option<String> = sqlx::query_scalar(
            "SELECT allowed_provider_key_ids FROM api_keys WHERE id = 'auth-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("scope should load");
        assert_eq!(scope, None);

        // Fault injection: corrupt the group scope, then delete key-2.
        sqlx::query(r#"UPDATE user_groups SET allowed_provider_key_ids = '{corrupt json' WHERE id = 'group-1'"#)
            .execute(&pool)
            .await
            .expect("corrupt scope should write");

        let result = repository
            .delete_provider_api_key_and_prune_scope("key-2")
            .await;
        assert!(result.is_err(), "corrupt scope must abort the delete");
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_api_keys WHERE id = 'key-2'")
                .fetch_one(&pool)
                .await
                .expect("count should load");
        assert_eq!(remaining, 1, "key deletion must roll back");
        let scope: Option<String> = sqlx::query_scalar(
            "SELECT allowed_provider_key_ids FROM api_keys WHERE id = 'auth-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("scope should load");
        assert_eq!(scope, None, "no partial prune may persist");
    }

    /// OAuth/CAS key deletion must prune scopes inside the same transaction.
    /// Requires a live MySQL at AETHER_TEST_MYSQL_URL; skipped when unset.
    #[tokio::test]
    async fn mysql_cas_oauth_key_delete_prunes_scopes_atomically_and_rolls_back_on_corrupt_scope() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL").ok() else {
            eprintln!("AETHER_TEST_MYSQL_URL unset; skipping mysql CAS test");
            return;
        };
        let server_url = database_url
            .rsplit_once('/')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_else(|| database_url.clone());
        let scope_url = format!("{server_url}/aether_test_cas_scope");
        let admin_pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .expect("mysql admin pool should connect");
        sqlx::query("DROP DATABASE IF EXISTS aether_test_cas_scope")
            .execute(&admin_pool)
            .await
            .expect("database should drop");
        sqlx::query("CREATE DATABASE aether_test_cas_scope")
            .execute(&admin_pool)
            .await
            .expect("database should create");
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&scope_url)
            .await
            .expect("mysql pool should connect");
        crate::run_migrations(&pool)
            .await
            .expect("migrations should run");

        sqlx::query(
            "INSERT INTO providers (id, name, provider_type, is_active, provider_priority, created_at, updated_at) VALUES ('provider-1', 'P1', 'custom', 1, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .expect("provider should seed");
        sqlx::query(
            "INSERT INTO users (id, email, email_verified, username, password_hash, role, auth_source, is_active, is_deleted, created_at, updated_at) VALUES ('user-1', 'u@example.com', 1, 'alice', NULL, 'user', 'local', 1, 0, 1, 1)",
        )
        .execute(&pool)
        .await
        .expect("user should seed");
        sqlx::query(
            r#"INSERT INTO provider_api_keys (id, provider_id, name, api_key, auth_type, is_active, internal_priority, total_tokens, total_cost_usd, created_at, updated_at) VALUES ('key-1', 'provider-1', 'one', 'enc-key', 'api_key', 1, 1, 0, 0, 1, 1)"#,
        )
        .execute(&pool)
        .await
        .expect("key should seed");
        sqlx::query(
            r#"INSERT INTO api_keys (id, user_id, key_hash, allowed_providers, allowed_provider_key_ids, rate_limit, is_active, is_standalone, total_requests, total_tokens, total_cost_usd, created_at, updated_at) VALUES ('auth-key-1', 'user-1', 'hash-1', '["provider-1"]', '{"provider-1":["key-1"]}', 100, 1, 1, 0, 0, 0, 1, 1)"#,
        )
        .execute(&pool)
        .await
        .expect("api key should seed");
        sqlx::query(
            r#"INSERT INTO user_groups (id, name, normalized_name, priority, allowed_providers, allowed_provider_key_ids, allowed_providers_mode, allowed_api_formats_mode, allowed_models_mode, rate_limit_mode, created_at, updated_at) VALUES ('group-1', 'G', 'g', 0, '["provider-1"]', '{"provider-1":["key-1"]}', 'specific', 'inherit', 'inherit', 'inherit', 1, 1)"#,
        )
        .execute(&pool)
        .await
        .expect("group should seed");

        let repository = MysqlProviderCatalogReadRepository::new(pool.clone());
        let cas_delete = aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyOAuthCredentialCasDelete {
            expected_upstream_metadata_namespace: None,
            key_id: "key-1".to_string(),
            expected_encrypted_auth_config: None,
            expected_credential: aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyOAuthCredentialFence {
                encrypted_api_key: Some("enc-key".to_string()),
                auth_type: "api_key".to_string(),
                provider_id: "provider-1".to_string(),
                provider_type: "custom".to_string(),
            },
        };

        let deleted = repository
            .compare_and_delete_key_oauth_credential(&cas_delete)
            .await
            .expect("cas delete should succeed");
        assert!(deleted);
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_api_keys WHERE id = 'key-1'")
                .fetch_one(&pool)
                .await
                .expect("count should load");
        assert_eq!(remaining, 0);
        let scope: Option<String> = sqlx::query_scalar(
            "SELECT allowed_provider_key_ids FROM api_keys WHERE id = 'auth-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("scope should load");
        assert_eq!(scope, None);
        let group_scope: Option<String> = sqlx::query_scalar(
            "SELECT allowed_provider_key_ids FROM user_groups WHERE id = 'group-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("group scope should load");
        assert_eq!(group_scope, None);

        // Corrupt scope row: the CAS delete must fail and roll back.
        sqlx::query(
            r#"INSERT INTO provider_api_keys (id, provider_id, name, api_key, auth_type, is_active, internal_priority, total_tokens, total_cost_usd, created_at, updated_at) VALUES ('key-2', 'provider-1', 'two', 'enc-key', 'api_key', 1, 1, 0, 0, 1, 1)"#,
        )
        .execute(&pool)
        .await
        .expect("key-2 should seed");
        sqlx::query(
            r#"UPDATE user_groups SET allowed_provider_key_ids = '{corrupt json' WHERE id = 'group-1'"#,
        )
        .execute(&pool)
        .await
        .expect("corrupt scope should write");
        let mut cas_delete_2 = cas_delete;
        cas_delete_2.key_id = "key-2".to_string();
        let result = repository
            .compare_and_delete_key_oauth_credential(&cas_delete_2)
            .await;
        assert!(result.is_err(), "corrupt scope must abort the CAS delete");
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM provider_api_keys WHERE id = 'key-2'")
                .fetch_one(&pool)
                .await
                .expect("count should load");
        assert_eq!(remaining, 1, "CAS delete must roll back");
    }
}
