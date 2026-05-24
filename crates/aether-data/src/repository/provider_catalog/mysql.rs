use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, Row};

use super::{
    InMemoryProviderCatalogReadRepository, ProviderCatalogKeyListQuery,
    ProviderCatalogReadRepository, ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogKeyMaintenanceSummary,
    StoredProviderCatalogKeyPage, StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use crate::driver::mysql::MysqlPool;
use crate::error::SqlResultExt;
use crate::DataLayerError;

#[derive(Debug, Clone)]
pub struct MysqlProviderCatalogReadRepository {
    pool: MysqlPool,
}

impl MysqlProviderCatalogReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn load_memory(&self) -> Result<InMemoryProviderCatalogReadRepository, DataLayerError> {
        Ok(InMemoryProviderCatalogReadRepository::seed(
            self.load_providers().await?,
            self.load_endpoints().await?,
            self.load_keys().await?,
        ))
    }

    async fn load_providers(&self) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        let rows = sqlx::query(
            r#"
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
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_provider_row).collect()
    }

    async fn load_endpoints(&self) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  id, provider_id, api_format, api_family, endpoint_kind, is_active,
  health_score, base_url, header_rules, body_rules, max_retries,
  custom_path, config, format_acceptance_config, proxy,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
FROM provider_endpoints
WHERE api_format IS NOT NULL
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_endpoint_row).collect()
    }

    async fn load_keys(&self) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        let rows = sqlx::query(
            r#"
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
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_key_row).collect()
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
        _endpoint_ids: &[String],
        _key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        validate_non_empty(provider_id, "provider catalog provider_id")?;
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
            .bind(optional_i64_from_u32(key.total_response_time_ms).unwrap_or(0))
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
        let rows_affected = sqlx::query(key_update_sql())
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
            .bind(optional_i64_from_u64(
                key.last_models_fetch_at_unix_secs,
                "provider_api_keys.last_models_fetch_at",
            )?)
            .bind(&key.last_models_fetch_error)
            .bind(updated_at)
            .bind(&key.id)
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
        self.load_memory().await?.list_providers(active_only).await
    }

    async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        self.load_memory()
            .await?
            .list_providers_by_ids(provider_ids)
            .await
    }

    async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        self.load_memory()
            .await?
            .list_endpoints_by_ids(endpoint_ids)
            .await
    }

    async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        self.load_memory()
            .await?
            .list_endpoints_by_provider_ids(provider_ids)
            .await
    }

    async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        self.load_memory().await?.list_keys_by_ids(key_ids).await
    }

    async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        self.load_memory()
            .await?
            .list_keys_by_provider_ids(provider_ids)
            .await
    }

    async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        self.load_memory()
            .await?
            .list_key_summaries_by_provider_ids(provider_ids)
            .await
    }

    async fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
        self.load_memory()
            .await?
            .list_key_maintenance_summaries_by_provider_ids(provider_ids)
            .await
    }

    async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
        self.load_memory().await?.list_keys_page(query).await
    }

    async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        self.load_memory()
            .await?
            .list_key_stats_by_provider_ids(provider_ids)
            .await
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
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        Self::cleanup_deleted_provider_refs(self, provider_id, endpoint_ids, key_ids).await
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
  learned_rpm_limit = ?,
  concurrent_429_count = ?,
  rpm_429_count = ?,
  last_429_at = ?,
  last_429_type = ?,
  adjustment_history = ?,
  utilization_samples = ?,
  last_probe_increase_at = ?,
  last_rpm_peak = ?,
  last_models_fetch_at = ?,
  last_models_fetch_error = ?,
  updated_at = ?
WHERE id = ?
"#
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
                optional_u32(
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
    use super::MysqlProviderCatalogReadRepository;
    use crate::lifecycle::migrate::run_mysql_migrations;
    use crate::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use serde_json::json;

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlProviderCatalogReadRepository::new(pool);
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
        run_mysql_migrations(&pool)
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

        let keys = repository
            .list_keys_by_provider_ids(std::slice::from_ref(&provider_id))
            .await
            .expect("keys should list");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].total_tokens, 1234);

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
}
