use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{
    postgres::{PgArguments, PgRow},
    query::Query,
    PgPool, Postgres, QueryBuilder, Row,
};

use aether_data_contracts::repository::provider_catalog::{
    patch_provider_catalog_runtime_credentials, provider_catalog_runtime_credentials_cas_matches,
    ProviderCatalogKeyAdaptiveStateUpdate, ProviderCatalogKeyAdminCasUpdate,
    ProviderCatalogKeyHealthStateUpdate, ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery,
    ProviderCatalogKeyOAuthCredentialCasDelete, ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ProviderCatalogKeyRuntimeMetadataUpdate, ProviderCatalogKeyStatusSnapshotUpdate,
    ProviderCatalogProviderConfigCasUpdate, ProviderCatalogReadRepository,
    ProviderCatalogRuntimeCredentialsCas, ProviderCatalogUpstreamMetadataNamespaceUpdate,
    ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
    StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use aether_data_contracts::DataLayerError;

use crate::error::{postgres_error, SqlxResultExt};
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
  COALESCE(api_key, encrypted_key) AS api_key,
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
  COALESCE(api_key, encrypted_key) AS api_key,
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
  NULL::bigint AS request_count,
  0::bigint AS total_tokens,
  0::double precision AS total_cost_usd,
  NULL::bigint AS success_count,
  NULL::bigint AS error_count,
  NULL::bigint AS total_response_time_ms,
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

const LIST_KEY_MAINTENANCE_SUMMARIES_BY_PROVIDER_IDS_PREFIX: &str = r#"
SELECT
  id,
  provider_id,
  is_active,
  upstream_metadata
FROM provider_api_keys
WHERE provider_id IN (
"#;

const KEY_UPDATE_SQL: &str = r#"
UPDATE provider_api_keys
SET
  provider_id = $2,
  api_formats = $3,
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
  allowed_models = $14,
  capabilities = $15,
  cache_ttl_minutes = $16,
  max_probe_interval_minutes = $17,
  auto_fetch_models = $18,
  locked_models = $19,
  model_include_patterns = $20,
  model_exclude_patterns = $21,
  proxy = $22,
  fingerprint = $23,
  expires_at = CASE
    WHEN $24::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($24::double precision)
  END,
  is_active = $25,
  updated_at = CASE
    WHEN $26::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($26::double precision)
  END,
  auth_type_by_format = $27,
  allow_auth_channel_mismatch_formats = $28
WHERE id = $1
  AND provider_id = $2
  AND auth_type = $4
  AND api_key IS NOT DISTINCT FROM $5
  AND auth_config IS NOT DISTINCT FROM $6
"#;

const KEY_RUNTIME_HEALTH_CAS_SQL: &str = r#"
UPDATE provider_api_keys
SET
  health_by_format = $2,
  circuit_breaker_by_format = $3,
  updated_at = NOW()
WHERE id = $1
  AND health_by_format::jsonb IS NOT DISTINCT FROM $4::jsonb
  AND circuit_breaker_by_format::jsonb IS NOT DISTINCT FROM $5::jsonb
  AND ($6::text IS NULL OR auth_config IS NOT DISTINCT FROM $6)
"#;

const KEY_RUNTIME_METADATA_CAS_SQL: &str = r#"
UPDATE provider_api_keys
SET
  upstream_metadata = COALESCE(upstream_metadata, '{}'::jsonb)
    || jsonb_build_object($2, $3::jsonb),
  status_snapshot = (COALESCE(status_snapshot::jsonb, '{}'::jsonb) || $4::jsonb)::json,
  updated_at = CASE
    WHEN $5::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($5::double precision)
  END
WHERE id = $1
  AND jsonb_typeof(COALESCE(upstream_metadata, '{}'::jsonb)) = 'object'
  AND (COALESCE(upstream_metadata, '{}'::jsonb) -> $2)
      IS NOT DISTINCT FROM $6::jsonb
"#;

fn validate_key_for_update(key: &StoredProviderCatalogKey) -> Result<(), DataLayerError> {
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
    if update.key_id.trim().is_empty() || update.namespace.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "provider catalog key_id and runtime metadata namespace are required".to_string(),
        ));
    }
    if !update.status_snapshot_patch.is_object() {
        return Err(DataLayerError::InvalidInput(
            "provider catalog runtime status snapshot patch must be an object".to_string(),
        ));
    }
    Ok(())
}

fn key_update_query(key: &StoredProviderCatalogKey) -> Query<'_, Postgres, PgArguments> {
    sqlx::query(KEY_UPDATE_SQL)
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
        .bind(key.expires_at_unix_secs.map(|value| value as f64))
        .bind(key.is_active)
        .bind(key.updated_at_unix_secs.map(|value| value as f64))
        .bind(&key.auth_type_by_format)
        .bind(&key.allow_auth_channel_mismatch_formats)
}

fn push_admin_key_assignments<'args>(
    builder: &mut QueryBuilder<'args, Postgres>,
    key: &'args StoredProviderCatalogKey,
) {
    builder
        .push_bind(&key.provider_id)
        .push(", api_formats = ")
        .push_bind(&key.api_formats)
        .push(", auth_type = ")
        .push_bind(&key.auth_type)
        .push(", api_key = ")
        .push_bind(&key.encrypted_api_key)
        .push(", auth_config = ")
        .push_bind(&key.encrypted_auth_config)
        .push(", name = ")
        .push_bind(&key.name)
        .push(", note = ")
        .push_bind(&key.note)
        .push(", rate_multipliers = ")
        .push_bind(&key.rate_multipliers)
        .push(", internal_priority = ")
        .push_bind(key.internal_priority)
        .push(", global_priority_by_format = ")
        .push_bind(&key.global_priority_by_format)
        .push(", rpm_limit = ")
        .push_bind(key.rpm_limit.map(|value| value as i32))
        .push(", concurrent_limit = ")
        .push_bind(key.concurrent_limit)
        .push(", allowed_models = ")
        .push_bind(&key.allowed_models)
        .push(", capabilities = ")
        .push_bind(&key.capabilities)
        .push(", cache_ttl_minutes = ")
        .push_bind(key.cache_ttl_minutes)
        .push(", max_probe_interval_minutes = ")
        .push_bind(key.max_probe_interval_minutes)
        .push(", auto_fetch_models = ")
        .push_bind(key.auto_fetch_models)
        .push(", locked_models = ")
        .push_bind(&key.locked_models)
        .push(", model_include_patterns = ")
        .push_bind(&key.model_include_patterns)
        .push(", model_exclude_patterns = ")
        .push_bind(&key.model_exclude_patterns)
        .push(", proxy = ")
        .push_bind(&key.proxy)
        .push(", fingerprint = ")
        .push_bind(&key.fingerprint)
        .push(", expires_at = CASE WHEN ")
        .push_bind(key.expires_at_unix_secs.map(|value| value as f64))
        .push("::double precision IS NULL THEN NULL ELSE TO_TIMESTAMP(")
        .push_bind(key.expires_at_unix_secs.map(|value| value as f64))
        .push("::double precision) END, is_active = ")
        .push_bind(key.is_active)
        .push(", updated_at = CASE WHEN ")
        .push_bind(key.updated_at_unix_secs.map(|value| value as f64))
        .push("::double precision IS NULL THEN NOW() ELSE TO_TIMESTAMP(")
        .push_bind(key.updated_at_unix_secs.map(|value| value as f64))
        .push("::double precision) END, auth_type_by_format = ")
        .push_bind(&key.auth_type_by_format)
        .push(", allow_auth_channel_mismatch_formats = ")
        .push_bind(&key.allow_auth_channel_mismatch_formats);
}

fn validate_admin_key_cas_update(
    update: &ProviderCatalogKeyAdminCasUpdate,
) -> Result<(), DataLayerError> {
    validate_key_for_update(&update.key)?;
    let expected = &update.expected_credential;
    if update.key.name.trim().is_empty()
        || update.key.auth_type.trim().is_empty()
        || expected.auth_type.trim().is_empty()
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
        let mut builder =
            QueryBuilder::<Postgres>::new(select_prefix_for_in(LIST_PROVIDERS_BY_IDS_PREFIX));
        let mut where_clause = WhereClause::new();
        if active_only {
            push_eq(&mut builder, &mut where_clause, "is_active", true);
        }
        builder.push(" ORDER BY provider_priority ASC, name ASC");
        collect_query_rows(builder.build().fetch(&self.pool), map_provider_row).await
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

    pub async fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            build_list_query(
                LIST_KEY_MAINTENANCE_SUMMARIES_BY_PROVIDER_IDS_PREFIX,
                provider_ids,
                " ORDER BY provider_id ASC, id ASC",
            )
            .build()
            .fetch(&self.pool),
            map_key_maintenance_summary_row,
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
        let order_by = match query.order {
            ProviderCatalogKeyListOrder::Name => "internal_priority ASC, name ASC, id ASC",
            ProviderCatalogKeyListOrder::CreatedAt => {
                "internal_priority ASC, COALESCE(created_at, TO_TIMESTAMP(0)) ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::CreatedAtAsc => {
                "created_at ASC NULLS LAST, name ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::CreatedAtDesc => {
                "created_at DESC NULLS LAST, name ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::LastUsedAtAsc => {
                "last_used_at ASC NULLS LAST, name ASC, id ASC"
            }
            ProviderCatalogKeyListOrder::LastUsedAtDesc => {
                "last_used_at DESC NULLS LAST, name ASC, id ASC"
            }
        };

        let mut count_builder = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*)::BIGINT AS total FROM provider_api_keys",
        );
        let mut count_where = WhereClause::new();
        apply_key_page_filters(&mut count_builder, &mut count_where, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
            .max(0) as usize;

        let mut list_builder =
            QueryBuilder::<Postgres>::new(select_prefix_for_in(LIST_KEYS_BY_IDS_PREFIX));
        let mut list_where = WhereClause::new();
        apply_key_page_filters(&mut list_builder, &mut list_where, query);
        list_builder.push(" ORDER BY ").push(order_by);
        push_limit_offset(&mut list_builder, limit, offset);
        let items = collect_query_rows(list_builder.build().fetch(&self.pool), map_key_row).await?;

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

    pub async fn update_key_oauth_runtime_state(
        &self,
        key_id: &str,
        oauth_invalid_at_unix_secs: Option<u64>,
        oauth_invalid_reason: Option<&str>,
        encrypted_auth_config_update: Option<&str>,
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
  oauth_invalid_at = CASE
    WHEN $2::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($2::double precision)
  END,
  oauth_invalid_reason = $3,
  auth_config = COALESCE($4, auth_config),
  updated_at = CASE
    WHEN $5::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($5::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(key_id)
        .bind(oauth_invalid_at_unix_secs.map(|value| value as f64))
        .bind(oauth_invalid_reason)
        .bind(encrypted_auth_config_update)
        .bind(updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn compare_and_update_key_oauth_runtime_state(
        &self,
        update: &ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty()
            || update.encrypted_auth_config.trim().is_empty()
            || update
                .encrypted_api_key_update
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            || update.expected_credential.as_ref().is_some_and(|expected| {
                expected
                    .encrypted_api_key
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                    || expected.auth_type.trim().is_empty()
                    || expected.provider_id.trim().is_empty()
                    || expected.provider_type.trim().is_empty()
            })
            || update
                .expected_upstream_metadata_namespace
                .as_ref()
                .is_some_and(|expected| expected.namespace.trim().is_empty())
            || update
                .upstream_metadata_namespace_to_remove
                .as_deref()
                .is_some_and(|namespace| namespace.trim().is_empty())
            || !update.status_snapshot_patch.is_object()
            || update
                .upstream_metadata_patch
                .as_ref()
                .is_some_and(|patch| !patch.is_object())
            || update
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
                "provider catalog OAuth runtime CAS requires key_id, auth_config, and object status patch"
                    .to_string(),
            ));
        }
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  oauth_invalid_at = CASE
    WHEN $2::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($2::double precision)
  END,
  oauth_invalid_reason = $3,
  auth_config = $4,
  api_key = CASE
    WHEN $5::text IS NULL THEN api_key
    ELSE $5
  END,
  expires_at = CASE
    WHEN $6::boolean IS FALSE THEN expires_at
    WHEN $7::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($7::double precision)
  END,
  upstream_metadata = CASE
    WHEN $8::jsonb IS NULL AND $9::text IS NULL THEN upstream_metadata
    WHEN $9::text IS NULL THEN COALESCE(upstream_metadata, '{}'::jsonb) || $8::jsonb
    ELSE (COALESCE(upstream_metadata, '{}'::jsonb) || COALESCE($8::jsonb, '{}'::jsonb)) - $9
  END,
  status_snapshot = (COALESCE(status_snapshot::jsonb, '{}'::jsonb) || $10::jsonb)::json,
  error_count = CASE WHEN $11::boolean THEN 0 ELSE error_count END,
  updated_at = CASE
    WHEN $12::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($12::double precision)
  END
WHERE id = $1
  AND auth_config IS NOT DISTINCT FROM $13
  AND (
    ($8::jsonb IS NULL AND $9::text IS NULL)
    OR jsonb_typeof(COALESCE(upstream_metadata, '{}'::jsonb)) = 'object'
  )
  AND ($14::boolean IS FALSE OR api_key IS NOT DISTINCT FROM $15)
  AND ($16::text IS NULL OR auth_type = $16)
  AND ($17::text IS NULL OR provider_id = $17)
  AND (
    $18::text IS NULL
    OR EXISTS (
      SELECT 1
      FROM providers
      WHERE providers.id = provider_api_keys.provider_id
        AND providers.provider_type = $18
    )
  )
  AND (
    $19::boolean IS FALSE
    OR (
      jsonb_typeof(COALESCE(upstream_metadata, '{}'::jsonb)) = 'object'
      AND (COALESCE(upstream_metadata, '{}'::jsonb) -> $20)
        IS NOT DISTINCT FROM $21::jsonb
    )
  )
"#,
        )
        .bind(&update.key_id)
        .bind(update.oauth_invalid_at_unix_secs.map(|value| value as f64))
        .bind(update.oauth_invalid_reason.as_deref())
        .bind(&update.encrypted_auth_config)
        .bind(update.encrypted_api_key_update.as_deref())
        .bind(update.expires_at_unix_secs_update.is_some())
        .bind(
            update
                .expires_at_unix_secs_update
                .flatten()
                .map(|value| value as f64),
        )
        .bind(update.upstream_metadata_patch.as_ref())
        .bind(update.upstream_metadata_namespace_to_remove.as_deref())
        .bind(&update.status_snapshot_patch)
        .bind(update.reset_error_count)
        .bind(update.updated_at_unix_secs.map(|value| value as f64))
        .bind(update.expected_encrypted_auth_config.as_deref())
        .bind(update.expected_credential.is_some())
        .bind(
            update
                .expected_credential
                .as_ref()
                .and_then(|expected| expected.encrypted_api_key.as_deref()),
        )
        .bind(
            update
                .expected_credential
                .as_ref()
                .map(|expected| expected.auth_type.as_str()),
        )
        .bind(
            update
                .expected_credential
                .as_ref()
                .map(|expected| expected.provider_id.as_str()),
        )
        .bind(
            update
                .expected_credential
                .as_ref()
                .map(|expected| expected.provider_type.as_str()),
        )
        .bind(update.expected_upstream_metadata_namespace.is_some())
        .bind(
            update
                .expected_upstream_metadata_namespace
                .as_ref()
                .map(|expected| expected.namespace.as_str()),
        )
        .bind(
            update
                .expected_upstream_metadata_namespace
                .as_ref()
                .and_then(|expected| expected.expected_value.as_ref()),
        )
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

    pub async fn compare_and_update_provider_config(
        &self,
        update: &ProviderCatalogProviderConfigCasUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider_id is empty".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let current_config = sqlx::query("SELECT config FROM providers WHERE id = $1 FOR UPDATE")
            .bind(&update.provider_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?
            .map(|row| row.try_get::<Option<serde_json::Value>, _>("config"))
            .transpose()
            .map_postgres_err()?;
        let Some(current_config) = current_config else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        if current_config != update.expected_config {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE providers SET config = $1::json, updated_at = CASE WHEN $2::double precision IS NULL THEN NOW() ELSE TO_TIMESTAMP($2::double precision) END WHERE id = $3",
        )
        .bind(&update.config)
        .bind(update.updated_at_unix_secs.map(|value| value as f64))
        .bind(&update.provider_id)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    pub async fn compare_and_patch_provider_ops_runtime_credentials(
        &self,
        update: &ProviderCatalogRuntimeCredentialsCas,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let Some(row) =
            sqlx::query("SELECT config, website, proxy FROM providers WHERE id = $1 FOR UPDATE")
                .bind(&update.provider_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?
        else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        let mut config: Option<serde_json::Value> = row.try_get("config").map_postgres_err()?;
        let website: Option<String> = row.try_get("website").map_postgres_err()?;
        let proxy: Option<serde_json::Value> = row.try_get("proxy").map_postgres_err()?;
        if !provider_catalog_runtime_credentials_cas_matches(
            config.as_ref(),
            website.as_deref(),
            proxy.as_ref(),
            update,
        ) {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        let Some(config) = config.as_mut() else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        patch_provider_catalog_runtime_credentials(config, &update.encrypted_credentials)?;
        sqlx::query("UPDATE providers SET config = $1::json, updated_at = NOW() WHERE id = $2")
            .bind(&*config)
            .bind(&update.provider_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        tx.commit().await.map_postgres_err()?;
        Ok(true)
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
        provider_deleted: bool,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog provider_id is empty".to_string(),
            ));
        }

        let mut tx = self.pool.begin().await.map_postgres_err()?;

        if provider_deleted {
            sqlx::query(
                "UPDATE user_preferences SET default_provider_id = NULL WHERE default_provider_id = $1",
            )
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
        }

        for endpoint_id in endpoint_ids {
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
            sqlx::query("UPDATE video_tasks SET key_id = NULL WHERE key_id = $1")
                .bind(key_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }

        if provider_deleted {
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
        }

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
  $26,
  CASE
    WHEN $27::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($27::double precision)
  END,
  CASE
    WHEN $28::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($28::double precision)
  END,
  $29,
  $30,
  COALESCE($31, 0),
  COALESCE($32, 0),
  CASE
    WHEN $33::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($33::double precision)
  END,
  $34,
  $35,
  $36,
  CASE
    WHEN $37::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($37::double precision)
  END,
  COALESCE($38, 0),
  COALESCE($39, 0),
  COALESCE($40, 0),
  COALESCE($41, 0),
  COALESCE($42, 0),
  COALESCE($43, 0),
  COALESCE($44, 0),
  CASE
    WHEN $45::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($45::double precision)
  END,
  CASE
    WHEN $46::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($46::double precision)
  END,
  $47,
  $48,
  $49,
  $50,
  CASE
    WHEN $51::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($51::double precision)
  END,
  CASE
    WHEN $52::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($52::double precision)
  END,
  $53
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
        .bind(key.request_count.map(i64::from))
        .bind(Some(i64::try_from(key.total_tokens).map_err(|_| {
            DataLayerError::InvalidInput(format!(
                "provider catalog key.total_tokens exceeds i64: {}",
                key.total_tokens
            ))
        })?))
        .bind(key.total_cost_usd)
        .bind(key.success_count.map(i64::from))
        .bind(key.error_count.map(i64::from))
        .bind(
            key.total_response_time_ms
                .map(|value| {
                    i64::try_from(value).map_err(|_| {
                        DataLayerError::InvalidInput(format!(
                            "provider catalog key.total_response_time_ms exceeds i64: {value}"
                        ))
                    })
                })
                .transpose()?,
        )
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
        validate_key_for_update(key)?;
        let rows_affected = key_update_query(key)
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

    pub async fn compare_and_update_key_admin_state(
        &self,
        update: &ProviderCatalogKeyAdminCasUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_admin_key_cas_update(update)?;
        let key = &update.key;
        let mut builder =
            QueryBuilder::<Postgres>::new("UPDATE provider_api_keys SET provider_id = ");
        push_admin_key_assignments(&mut builder, key);
        if update.reset_oauth_runtime {
            builder.push(", oauth_invalid_at = NULL, oauth_invalid_reason = NULL, error_count = 0");
        }
        if let Some(rotation) = update.codex_rotation.as_ref() {
            builder
                .push(", upstream_metadata = jsonb_set(COALESCE(upstream_metadata, '{}'::jsonb), '{codex}', ")
                .push_bind(rotation)
                .push("::jsonb, true)");
        }
        if update.codex_rotation.is_some() || update.reset_oauth_runtime {
            builder.push(", status_snapshot = ");
            match (update.codex_rotation.is_some(), update.reset_oauth_runtime) {
                (true, true) => builder.push("jsonb_set(jsonb_set(COALESCE(status_snapshot::jsonb, '{}'::jsonb), '{quota}', 'null'::jsonb, true), '{oauth}', 'null'::jsonb, true)::json"),
                (true, false) => builder.push("jsonb_set(COALESCE(status_snapshot::jsonb, '{}'::jsonb), '{quota}', 'null'::jsonb, true)::json"),
                (false, true) => builder.push("jsonb_set(COALESCE(status_snapshot::jsonb, '{}'::jsonb), '{oauth}', 'null'::jsonb, true)::json"),
                (false, false) => unreachable!(),
            };
        }
        builder
            .push(" WHERE id = ")
            .push_bind(&key.id)
            .push(" AND api_key IS NOT DISTINCT FROM ")
            .push_bind(update.expected_credential.encrypted_api_key.as_deref())
            .push(" AND auth_config IS NOT DISTINCT FROM ")
            .push_bind(update.expected_encrypted_auth_config.as_deref())
            .push(" AND auth_type = ")
            .push_bind(&update.expected_credential.auth_type)
            .push(" AND provider_id = ")
            .push_bind(&update.expected_credential.provider_id)
            .push(
                " AND EXISTS (SELECT 1 FROM providers WHERE providers.id = provider_api_keys.provider_id AND providers.provider_type = ",
            )
            .push_bind(&update.expected_credential.provider_type)
            .push(")");
        if update.codex_rotation.is_some() {
            builder
                .push(" AND jsonb_typeof(COALESCE(upstream_metadata, '{}'::jsonb)) = 'object'")
                .push(" AND NOT (api_key IS NOT DISTINCT FROM ")
                .push_bind(key.encrypted_api_key.as_deref())
                .push(" AND auth_config IS NOT DISTINCT FROM ")
                .push_bind(key.encrypted_auth_config.as_deref())
                .push(" AND auth_type = ")
                .push_bind(&key.auth_type)
                .push(" AND provider_id = ")
                .push_bind(&key.provider_id)
                .push(")");
        }
        if update.codex_rotation.is_some() || update.reset_oauth_runtime {
            builder.push(
                " AND jsonb_typeof(COALESCE(status_snapshot::jsonb, '{}'::jsonb)) = 'object'",
            );
        }
        let rows_affected = builder
            .build()
            .execute(&self.pool)
            .await
            .map_postgres_err()?
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
            validate_key_for_update(key)?;
        }

        let mut transaction = self.pool.begin().await.map_postgres_err()?;
        for key in keys {
            let rows_affected = key_update_query(key)
                .execute(&mut *transaction)
                .await
                .map_postgres_err()?
                .rows_affected();
            if rows_affected == 0 {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "provider catalog key {} not found",
                    key.id
                )));
            }
        }
        transaction.commit().await.map_postgres_err()?;
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

    pub async fn compare_and_delete_key_oauth_credential(
        &self,
        delete: &ProviderCatalogKeyOAuthCredentialCasDelete,
    ) -> Result<bool, DataLayerError> {
        let expected = &delete.expected_credential;
        if delete.key_id.trim().is_empty()
            || expected
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
        let rows_affected = sqlx::query(
            r#"
DELETE FROM provider_api_keys
WHERE id = $1
  AND auth_config IS NOT DISTINCT FROM $2
  AND api_key IS NOT DISTINCT FROM $3
  AND auth_type = $4
  AND provider_id = $5
  AND EXISTS (
    SELECT 1
    FROM providers
    WHERE providers.id = provider_api_keys.provider_id
      AND providers.provider_type = $6
  )
  AND (
    $7::boolean IS FALSE
    OR (
      jsonb_typeof(COALESCE(upstream_metadata, '{}'::jsonb)) = 'object'
      AND (COALESCE(upstream_metadata, '{}'::jsonb) -> $8)
        IS NOT DISTINCT FROM $9::jsonb
    )
  )
"#,
        )
        .bind(&delete.key_id)
        .bind(delete.expected_encrypted_auth_config.as_deref())
        .bind(expected.encrypted_api_key.as_deref())
        .bind(&expected.auth_type)
        .bind(&expected.provider_id)
        .bind(&expected.provider_type)
        .bind(delete.expected_upstream_metadata_namespace.is_some())
        .bind(
            delete
                .expected_upstream_metadata_namespace
                .as_ref()
                .map(|expected| expected.namespace.as_str()),
        )
        .bind(
            delete
                .expected_upstream_metadata_namespace
                .as_ref()
                .and_then(|expected| expected.expected_value.as_ref()),
        )
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

    pub async fn upsert_key_upstream_metadata_namespace(
        &self,
        key_id: &str,
        namespace: &str,
        value: &serde_json::Value,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        if key_id.trim().is_empty() || namespace.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id and upstream metadata namespace are required".to_string(),
            ));
        }
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  upstream_metadata = COALESCE(upstream_metadata, '{}'::jsonb) || jsonb_build_object($2, $3::jsonb),
  updated_at = CASE
    WHEN $4::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($4::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(key_id)
        .bind(namespace)
        .bind(value)
        .bind(updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
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
        if key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  allowed_models = $2,
  last_models_fetch_at = CASE
    WHEN $3::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($3::double precision)
  END,
  last_models_fetch_error = $4,
  updated_at = CASE
    WHEN $5::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($5::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(key_id)
        .bind(allowed_models)
        .bind(last_models_fetch_at_unix_secs.map(|value| value as f64))
        .bind(last_models_fetch_error)
        .bind(updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
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
        if key_id.trim().is_empty()
            || upstream_metadata_updates
                .iter()
                .any(|update| update.namespace.trim().is_empty())
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id and upstream metadata namespaces are required".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  allowed_models = $2,
  last_models_fetch_at = TO_TIMESTAMP($3::double precision),
  last_models_fetch_error = NULL,
  updated_at = CASE
    WHEN $4::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($4::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(key_id)
        .bind(allowed_models)
        .bind(last_models_fetch_at_unix_secs as f64)
        .bind(updated_at_unix_secs.map(|value| value as f64))
        .execute(&mut *tx)
        .await
        .map_postgres_err()?
        .rows_affected();
        if rows_affected == 0 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        for update in upstream_metadata_updates {
            sqlx::query(
                r#"
UPDATE provider_api_keys
SET upstream_metadata = COALESCE(upstream_metadata, '{}'::jsonb)
      || jsonb_build_object($2, $3::jsonb)
WHERE id = $1
"#,
            )
            .bind(key_id)
            .bind(&update.namespace)
            .bind(&update.value)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }
        tx.commit().await.map_postgres_err()?;
        Ok(true)
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

    pub async fn reset_key_error_count(&self, key_id: &str) -> Result<bool, DataLayerError> {
        if key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }

        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET error_count = 0, updated_at = NOW()
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

    pub async fn compare_and_update_key_adaptive_state(
        &self,
        update: &ProviderCatalogKeyAdaptiveStateUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }
        let status_snapshot_patch = adaptive_status_snapshot_patch(&update.status_snapshot_patch)?;
        let expected = update.expected.canonicalized();
        let next = update.next.canonicalized();
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  learned_rpm_limit = $2,
  rpm_429_count = $3,
  last_429_at = CASE
    WHEN $4::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($4::double precision)
  END,
  last_429_type = $5,
  adjustment_history = $6,
  utilization_samples = $7,
  last_probe_increase_at = CASE
    WHEN $8::double precision IS NULL THEN NULL
    ELSE TO_TIMESTAMP($8::double precision)
  END,
  last_rpm_peak = $9,
  concurrent_429_count = $10,
  status_snapshot = (COALESCE(status_snapshot::jsonb, '{}'::jsonb) || $11::jsonb)::json,
  updated_at = CASE
    WHEN $12::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($12::double precision)
  END
WHERE id = $1
  AND learned_rpm_limit IS NOT DISTINCT FROM $13
  AND rpm_429_count IS NOT DISTINCT FROM $14
  AND CAST(EXTRACT(EPOCH FROM last_429_at) AS BIGINT) IS NOT DISTINCT FROM $15
  AND last_429_type IS NOT DISTINCT FROM $16
  AND adjustment_history::jsonb IS NOT DISTINCT FROM $17::jsonb
  AND utilization_samples::jsonb IS NOT DISTINCT FROM $18::jsonb
  AND CAST(EXTRACT(EPOCH FROM last_probe_increase_at) AS BIGINT) IS NOT DISTINCT FROM $19
  AND last_rpm_peak IS NOT DISTINCT FROM $20
  AND concurrent_429_count IS NOT DISTINCT FROM $21
  AND ($22::text IS NULL OR auth_config IS NOT DISTINCT FROM $22)
"#,
        )
        .bind(&update.key_id)
        .bind(next.learned_rpm_limit.map(|value| value as i32))
        .bind(next.rpm_429_count.map(|value| value as i32))
        .bind(next.last_429_at_unix_secs.map(|value| value as f64))
        .bind(&next.last_429_type)
        .bind(&next.adjustment_history)
        .bind(&next.utilization_samples)
        .bind(
            next.last_probe_increase_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(next.last_rpm_peak.map(|value| value as i32))
        .bind(next.concurrent_429_count.map(|value| value as i32))
        .bind(&status_snapshot_patch)
        .bind(update.updated_at_unix_secs.map(|value| value as f64))
        .bind(expected.learned_rpm_limit.map(|value| value as i32))
        .bind(expected.rpm_429_count.map(|value| value as i32))
        .bind(
            expected
                .last_429_at_unix_secs
                .and_then(|value| i64::try_from(value).ok()),
        )
        .bind(&expected.last_429_type)
        .bind(&expected.adjustment_history)
        .bind(&expected.utilization_samples)
        .bind(
            expected
                .last_probe_increase_at_unix_secs
                .and_then(|value| i64::try_from(value).ok()),
        )
        .bind(expected.last_rpm_peak.map(|value| value as i32))
        .bind(expected.concurrent_429_count.map(|value| value as i32))
        .bind(update.expected_encrypted_auth_config.as_deref())
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_runtime_metadata(
        &self,
        update: &ProviderCatalogKeyRuntimeMetadataUpdate,
    ) -> Result<bool, DataLayerError> {
        validate_runtime_metadata_update(update)?;
        let rows_affected = sqlx::query(KEY_RUNTIME_METADATA_CAS_SQL)
            .bind(&update.key_id)
            .bind(&update.namespace)
            .bind(&update.upstream_metadata_value)
            .bind(&update.status_snapshot_patch)
            .bind(update.updated_at_unix_secs.map(|value| value as f64))
            .bind(update.expected_upstream_metadata_value.as_ref())
            .execute(&self.pool)
            .await
            .map_postgres_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn update_key_status_snapshot(
        &self,
        update: &ProviderCatalogKeyStatusSnapshotUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty() || !update.status_snapshot_patch.is_object() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id and object status snapshot patch are required".to_string(),
            ));
        }
        let rows_affected = sqlx::query(
            r#"
UPDATE provider_api_keys
SET
  status_snapshot = (COALESCE(status_snapshot::jsonb, '{}'::jsonb) || $2::jsonb)::json,
  updated_at = CASE
    WHEN $3::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($3::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(&update.key_id)
        .bind(&update.status_snapshot_patch)
        .bind(update.updated_at_unix_secs.map(|value| value as f64))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    pub async fn compare_and_update_key_health_state(
        &self,
        update: &ProviderCatalogKeyHealthStateUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }
        let rows_affected = sqlx::query(KEY_RUNTIME_HEALTH_CAS_SQL)
            .bind(&update.key_id)
            .bind(&update.health_by_format)
            .bind(&update.circuit_breaker_by_format)
            .bind(&update.expected_health_by_format)
            .bind(&update.expected_circuit_breaker_by_format)
            .bind(update.expected_encrypted_auth_config.as_deref())
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

    async fn compare_and_update_provider_config(
        &self,
        update: &ProviderCatalogProviderConfigCasUpdate,
    ) -> Result<bool, DataLayerError> {
        Self::compare_and_update_provider_config(self, update).await
    }

    async fn compare_and_patch_provider_ops_runtime_credentials(
        &self,
        update: &ProviderCatalogRuntimeCredentialsCas,
    ) -> Result<bool, DataLayerError> {
        Self::compare_and_patch_provider_ops_runtime_credentials(self, update).await
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

fn build_list_query<'a>(
    prefix: &'static str,
    ids: &'a [String],
    suffix: &'static str,
) -> QueryBuilder<'a, Postgres> {
    let mut builder = QueryBuilder::<Postgres>::new(select_prefix_for_in(prefix));
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
    builder: &mut QueryBuilder<'a, Postgres>,
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
            SqlDialect::Postgres,
            &["name", "id"],
            search,
        );
    }
    push_optional_eq(builder, where_clause, "is_active", query.is_active);
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

fn map_key_maintenance_summary_row(
    row: &PgRow,
) -> Result<StoredProviderCatalogKeyMaintenanceSummary, DataLayerError> {
    Ok(StoredProviderCatalogKeyMaintenanceSummary {
        id: row_get(row, "id")?,
        provider_id: row_get(row, "provider_id")?,
        is_active: row_get(row, "is_active")?,
        upstream_metadata: row_get(row, "upstream_metadata")?,
    })
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
    let request_count = row_get::<Option<i64>>(row, "request_count")?
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
    let success_count = row_get::<Option<i64>>(row, "success_count")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.success_count: {value}"
                ))
            })
        })
        .transpose()?;
    let error_count = row_get::<Option<i64>>(row, "error_count")?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider_api_keys.error_count: {value}"
                ))
            })
        })
        .transpose()?;
    let total_response_time_ms = row_get::<Option<i64>>(row, "total_response_time_ms")?
        .map(|value| {
            u64::try_from(value).map_err(|_| {
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
    use crate::{PostgresPoolConfig, PostgresPoolFactory, SqlxProviderQuotaRepository};
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogProviderConfigCasUpdate, ProviderCatalogRuntimeCredentialsCas,
        StoredProviderCatalogProvider,
    };
    use aether_data_contracts::repository::quota::{
        ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch,
        ProviderQuotaReadRepository, ProviderQuotaUsageObservation, ProviderQuotaWriteRepository,
    };
    use serde_json::json;

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

    #[tokio::test]
    async fn postgres_provider_config_and_remote_quota_cas_when_url_is_set() {
        let Some(database_url) = std::env::var("AETHER_TEST_POSTGRES_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!("skipping postgres provider CAS smoke test: AETHER_TEST_POSTGRES_URL unset");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("postgres test pool should connect");
        crate::run_migrations(&pool)
            .await
            .expect("postgres migrations should run");
        let provider_id = format!("provider-cas-{}", uuid::Uuid::new_v4().simple());
        let mut provider = StoredProviderCatalogProvider::new(
            provider_id.clone(),
            "Provider CAS".to_string(),
            Some("https://example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build");
        provider.billing_type = Some("monthly_quota".to_string());
        provider.monthly_quota_usd = Some(100.0);
        provider.monthly_used_usd = Some(7.0);
        provider.quota_reset_day = Some(7);
        provider.quota_last_reset_at_unix_secs = Some(1_710_000_000);
        provider.config = Some(json!({
            "provider_ops": {
                "connector": {"credentials": {"refresh_token": "old-token"}}
            },
            "feature_flag": false
        }));
        let repository = SqlxProviderCatalogReadRepository::new(pool.clone());
        let created = repository
            .create_provider(&provider, None)
            .await
            .expect("provider should create");
        assert!(repository
            .compare_and_patch_provider_ops_runtime_credentials(
                &ProviderCatalogRuntimeCredentialsCas {
                    provider_id: provider_id.clone(),
                    expected_provider_config: created.config.clone(),
                    expected_provider_website: created.website.clone(),
                    expected_provider_proxy: created.proxy.clone(),
                    encrypted_credentials: serde_json::Map::from_iter([(
                        "refresh_token".to_string(),
                        json!("new-token"),
                    )]),
                },
            )
            .await
            .expect("runtime credential patch should succeed"));
        assert!(!repository
            .compare_and_update_provider_config(&ProviderCatalogProviderConfigCasUpdate {
                provider_id: provider_id.clone(),
                expected_config: created.config,
                config: Some(json!({"feature_flag": true})),
                updated_at_unix_secs: Some(1_720_000_000),
            })
            .await
            .expect("stale provider config should be rejected"));
        let latest = repository
            .list_providers_by_ids(std::slice::from_ref(&provider_id))
            .await
            .expect("provider should reload")
            .pop()
            .expect("provider should exist");
        let mut next_config = latest.config.clone().expect("config should exist");
        next_config["feature_flag"] = json!(true);
        assert!(repository
            .compare_and_update_provider_config(&ProviderCatalogProviderConfigCasUpdate {
                provider_id: provider_id.clone(),
                expected_config: latest.config,
                config: Some(next_config),
                updated_at_unix_secs: Some(1_720_000_001),
            })
            .await
            .expect("fresh provider config should update"));

        let quota_repository = SqlxProviderQuotaRepository::new(pool);
        let remote_patch = ApplyRemoteProviderQuotaPatch {
            provider_id: provider_id.clone(),
            billing_type: "monthly_quota".to_string(),
            monthly_quota_usd: Some(50.0),
            remote_monthly_used_usd: 3.0,
            remote_window_start_unix_secs: 1_720_000_000,
            remote_window_end_unix_secs: 1_720_604_800,
            quota_reset_day: Some(7),
            quota_expires_at_unix_secs: Some(1_730_000_000),
            local_usage_observation: Some(ProviderQuotaUsageObservation {
                monthly_used_usd: 7.0,
                quota_last_reset_at_unix_secs: Some(1_710_000_000),
            }),
            preserve_local_used_usd: false,
        };
        quota_repository
            .apply_remote_provider_quota(&remote_patch)
            .await
            .expect("remote quota should apply");
        quota_repository
            .apply_remote_provider_quota(&remote_patch)
            .await
            .expect("repeated remote quota patch should be idempotent");
        assert!(matches!(
            quota_repository
                .apply_remote_provider_quota(&ApplyRemoteProviderQuotaPatch {
                    remote_monthly_used_usd: 4.0,
                    ..remote_patch.clone()
                })
                .await
                .expect("divergent remote quota patch should classify"),
            ApplyRemoteProviderQuotaOutcome::ConcurrentModification(_)
        ));
        let stored = quota_repository
            .find_by_provider_id(&provider_id)
            .await
            .expect("quota should reload")
            .expect("quota should exist");
        assert_eq!(stored.monthly_used_usd, 3.0);
        repository
            .delete_provider(&provider_id)
            .await
            .expect("provider cleanup should succeed");
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

        let source = include_str!("provider_catalog.rs").replace("\r\n", "\n");
        assert!(source.contains("concurrent_limit,"));
        assert!(source.contains("concurrent_limit = $13"));
        assert!(source.contains(".bind(key.concurrent_limit)"));
        assert!(source.contains("row_get::<Option<i32>>(row, \"concurrent_limit\")"));
    }

    #[test]
    fn provider_api_keys_concurrent_limit_schema_is_nullable_without_default() {
        let migration = include_str!(
            "../migrations/20260502000000_add_provider_key_auth_channel_mismatch_formats.sql"
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

        let baseline = include_str!("../migrations/20260403000000_baseline.sql");
        assert!(baseline.contains("CREATE TABLE IF NOT EXISTS public.provider_api_keys"));
        assert!(baseline.contains("concurrent_limit integer,"));
    }

    #[test]
    fn provider_api_keys_auth_channel_mismatch_list_queries_include_field() {
        for sql in [
            super::LIST_KEYS_BY_IDS_PREFIX,
            super::LIST_KEYS_BY_PROVIDER_IDS_PREFIX,
        ] {
            assert!(sql.contains("allow_auth_channel_mismatch_formats"));
        }

        let source = include_str!("provider_catalog.rs").replace("\r\n", "\n");
        assert!(
            source
                .matches(
                    "auth_type_by_format,\n  allow_auth_channel_mismatch_formats,\n  COALESCE(api_key, encrypted_key) AS api_key",
                )
                .count()
                >= 2
        );
        assert!(source.contains("QueryBuilder::<Postgres>::new(select_prefix_for_in("));
        assert!(source.contains(".bind(&key.allow_auth_channel_mismatch_formats)"));
        assert!(source.contains("row.try_get(\"allow_auth_channel_mismatch_formats\").ok()"));
    }

    #[test]
    fn provider_api_keys_create_key_insert_placeholders_match_bind_order() {
        let source = include_str!("provider_catalog.rs").replace("\r\n", "\n");
        assert!(source.contains(
            "  $24,\n  $25,\n  $26,\n  CASE\n    WHEN $27::double precision IS NULL THEN NULL"
        ));
        assert!(source.contains("  $29,\n  $30,\n  COALESCE($31, 0),"));
        assert!(source.contains(
            "  COALESCE($42, 0),\n  COALESCE($43, 0),\n  COALESCE($44, 0),\n  CASE\n    WHEN $45::double precision IS NULL THEN NULL"
        ));
        assert!(source.contains(
            "  CASE\n    WHEN $52::double precision IS NULL THEN NOW()\n    ELSE TO_TIMESTAMP($52::double precision)\n  END,\n  $53"
        ));
    }

    #[test]
    fn runtime_health_cas_never_updates_admin_activation() {
        let sql = super::KEY_RUNTIME_HEALTH_CAS_SQL.to_ascii_lowercase();
        assert!(sql.contains("health_by_format = $2"));
        assert!(sql.contains("circuit_breaker_by_format = $3"));
        assert!(sql.contains("health_by_format::jsonb is not distinct from $4::jsonb"));
        assert!(!sql.contains("is_active"));
    }

    #[test]
    fn runtime_metadata_cas_compares_only_the_requested_namespace() {
        let sql = super::KEY_RUNTIME_METADATA_CAS_SQL.to_ascii_lowercase();
        assert!(sql.contains("upstream_metadata, '{}'::jsonb) -> $2"));
        assert!(sql.contains("jsonb_typeof(coalesce(upstream_metadata, '{}'::jsonb)) = 'object'"));
        assert!(sql.contains("is not distinct from $6::jsonb"));
        assert!(sql.contains("status_snapshot::jsonb"));
        assert!(!sql.contains("is_active"));
    }

    #[test]
    fn ordinary_key_update_does_not_own_runtime_observation_fields() {
        let sql = super::KEY_UPDATE_SQL.to_ascii_lowercase();
        for runtime_assignment in [
            "learned_rpm_limit =",
            "rpm_429_count =",
            "last_429_at =",
            "last_429_type =",
            "adjustment_history =",
            "utilization_samples =",
            "last_probe_increase_at =",
            "last_rpm_peak =",
            "upstream_metadata =",
            "status_snapshot =",
            "health_by_format =",
            "circuit_breaker_by_format =",
        ] {
            assert!(
                !sql.contains(runtime_assignment),
                "ordinary key update unexpectedly owns {runtime_assignment}"
            );
        }
        assert!(sql.contains("is_active = $25"));
        assert!(sql.contains("rpm_limit = $12"));
        assert!(sql.contains("api_key is not distinct from $5"));
        assert!(sql.contains("auth_config is not distinct from $6"));
    }

    #[test]
    fn admin_credential_cas_has_atomic_rotation_guards() {
        let source = include_str!("provider_catalog.rs");
        for predicate in [
            "api_key IS NOT DISTINCT FROM ",
            "auth_config IS NOT DISTINCT FROM ",
            "jsonb_typeof(COALESCE(upstream_metadata, '{}'::jsonb)) = 'object'",
            "jsonb_typeof(COALESCE(status_snapshot::jsonb, '{}'::jsonb)) = 'object'",
            "jsonb_set(COALESCE(upstream_metadata, '{}'::jsonb), '{codex}'",
            "jsonb_set(COALESCE(status_snapshot::jsonb, '{}'::jsonb), '{quota}', 'null'::jsonb, true)::json",
            "oauth_invalid_at = NULL, oauth_invalid_reason = NULL, error_count = 0",
        ] {
            assert!(
                source.contains(predicate),
                "missing admin CAS guard: {predicate}"
            );
        }
    }
}
