use async_trait::async_trait;
use sqlx::{PgPool, Row};

use aether_data_contracts::repository::billing::{
    AdminBillingCollectorRecord, AdminBillingCollectorWriteInput, AdminBillingMutationOutcome,
    AdminBillingPresetApplyResult, AdminBillingRuleRecord, AdminBillingRuleWriteInput,
    BillingPlanRecord, BillingPlanWriteInput, BillingReadRepository, PaymentGatewayConfigRecord,
    PaymentGatewayConfigWriteInput, StoredBillingModelContext, UserDailyQuotaAvailabilityRecord,
    UserPlanEntitlementRecord,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlxResultExt;

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

    async fn admin_billing_enabled_default_value_exists(
        &self,
        api_format: &str,
        task_type: &str,
        dimension_name: &str,
        existing_id: Option<&str>,
    ) -> Result<Option<bool>, DataLayerError> {
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
SELECT EXISTS(
  SELECT 1
  FROM dimension_collectors
  WHERE api_format = $1
    AND task_type = $2
    AND dimension_name = $3
    AND is_enabled = TRUE
    AND default_value IS NOT NULL
    AND ($4::TEXT IS NULL OR id <> $4)
)
            "#,
        )
        .bind(api_format)
        .bind(task_type)
        .bind(dimension_name)
        .bind(existing_id)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(Some(exists))
    }

    async fn create_admin_billing_rule(
        &self,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, DataLayerError> {
        let rule_id = uuid::Uuid::new_v4().to_string();
        let row = match sqlx::query(
            r#"
INSERT INTO billing_rules (
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW(), NOW())
RETURNING
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
            "#,
        )
        .bind(&rule_id)
        .bind(&input.name)
        .bind(&input.task_type)
        .bind(input.global_model_id.as_deref())
        .bind(input.model_id.as_deref())
        .bind(&input.expression)
        .bind(&input.variables)
        .bind(&input.dimension_mappings)
        .bind(input.is_enabled)
        .fetch_one(&self.pool)
        .await
        {
            Ok(row) => row,
            Err(sqlx::Error::Database(err)) => {
                return Ok(AdminBillingMutationOutcome::Invalid(format!(
                    "Integrity error: {err}"
                )))
            }
            Err(err) => return Err(DataLayerError::postgres(err)),
        };
        Ok(AdminBillingMutationOutcome::Applied(
            map_admin_billing_rule_row(&row)?,
        ))
    }

    async fn list_admin_billing_rules(
        &self,
        task_type: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingRuleRecord>, u64)>, DataLayerError> {
        let total = read_count(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM billing_rules
WHERE ($1::TEXT IS NULL OR task_type = $1)
  AND ($2::BOOL IS NULL OR is_enabled = $2)
                "#,
            )
            .bind(task_type)
            .bind(is_enabled)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
        )?;
        let offset = u64::from(page.saturating_sub(1) * page_size);
        let rows = sqlx::query(
            r#"
SELECT
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM billing_rules
WHERE ($1::TEXT IS NULL OR task_type = $1)
  AND ($2::BOOL IS NULL OR is_enabled = $2)
ORDER BY updated_at DESC
OFFSET $3
LIMIT $4
            "#,
        )
        .bind(task_type)
        .bind(is_enabled)
        .bind(
            i64::try_from(offset)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        )
        .bind(i64::from(page_size))
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        let items = rows
            .iter()
            .map(map_admin_billing_rule_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((items, total)))
    }

    async fn find_admin_billing_rule(
        &self,
        rule_id: &str,
    ) -> Result<Option<AdminBillingRuleRecord>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM billing_rules
WHERE id = $1
            "#,
        )
        .bind(rule_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref().map(map_admin_billing_rule_row).transpose()
    }

    async fn update_admin_billing_rule(
        &self,
        rule_id: &str,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, DataLayerError> {
        let row = match sqlx::query(
            r#"
UPDATE billing_rules
SET
  name = $2,
  task_type = $3,
  global_model_id = $4,
  model_id = $5,
  expression = $6,
  variables = $7,
  dimension_mappings = $8,
  is_enabled = $9,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
            "#,
        )
        .bind(rule_id)
        .bind(&input.name)
        .bind(&input.task_type)
        .bind(input.global_model_id.as_deref())
        .bind(input.model_id.as_deref())
        .bind(&input.expression)
        .bind(&input.variables)
        .bind(&input.dimension_mappings)
        .bind(input.is_enabled)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(row) => row,
            Err(sqlx::Error::Database(err)) => {
                return Ok(AdminBillingMutationOutcome::Invalid(format!(
                    "Integrity error: {err}"
                )))
            }
            Err(err) => return Err(DataLayerError::postgres(err)),
        };
        match row {
            Some(row) => Ok(AdminBillingMutationOutcome::Applied(
                map_admin_billing_rule_row(&row)?,
            )),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }

    async fn create_admin_billing_collector(
        &self,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, DataLayerError> {
        let collector_id = uuid::Uuid::new_v4().to_string();
        let row = match sqlx::query(
            r#"
INSERT INTO dimension_collectors (
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(), NOW())
RETURNING
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
            "#,
        )
        .bind(&collector_id)
        .bind(&input.api_format)
        .bind(&input.task_type)
        .bind(&input.dimension_name)
        .bind(&input.source_type)
        .bind(input.source_path.as_deref())
        .bind(&input.value_type)
        .bind(input.transform_expression.as_deref())
        .bind(input.default_value.as_deref())
        .bind(input.priority)
        .bind(input.is_enabled)
        .fetch_one(&self.pool)
        .await
        {
            Ok(row) => row,
            Err(sqlx::Error::Database(err)) => {
                return Ok(AdminBillingMutationOutcome::Invalid(format!(
                    "Integrity error: {err}"
                )))
            }
            Err(err) => return Err(DataLayerError::postgres(err)),
        };
        Ok(AdminBillingMutationOutcome::Applied(
            map_admin_billing_collector_row(&row)?,
        ))
    }

    async fn list_admin_billing_collectors(
        &self,
        api_format: Option<&str>,
        task_type: Option<&str>,
        dimension_name: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingCollectorRecord>, u64)>, DataLayerError> {
        let total = read_count(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM dimension_collectors
WHERE ($1::TEXT IS NULL OR api_format = $1)
  AND ($2::TEXT IS NULL OR task_type = $2)
  AND ($3::TEXT IS NULL OR dimension_name = $3)
  AND ($4::BOOL IS NULL OR is_enabled = $4)
                "#,
            )
            .bind(api_format)
            .bind(task_type)
            .bind(dimension_name)
            .bind(is_enabled)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
        )?;
        let offset = u64::from(page.saturating_sub(1) * page_size);
        let rows = sqlx::query(
            r#"
SELECT
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM dimension_collectors
WHERE ($1::TEXT IS NULL OR api_format = $1)
  AND ($2::TEXT IS NULL OR task_type = $2)
  AND ($3::TEXT IS NULL OR dimension_name = $3)
  AND ($4::BOOL IS NULL OR is_enabled = $4)
ORDER BY updated_at DESC, priority DESC, id ASC
OFFSET $5
LIMIT $6
            "#,
        )
        .bind(api_format)
        .bind(task_type)
        .bind(dimension_name)
        .bind(is_enabled)
        .bind(
            i64::try_from(offset)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        )
        .bind(i64::from(page_size))
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        let items = rows
            .iter()
            .map(map_admin_billing_collector_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((items, total)))
    }

    async fn find_admin_billing_collector(
        &self,
        collector_id: &str,
    ) -> Result<Option<AdminBillingCollectorRecord>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM dimension_collectors
WHERE id = $1
            "#,
        )
        .bind(collector_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref()
            .map(map_admin_billing_collector_row)
            .transpose()
    }

    async fn update_admin_billing_collector(
        &self,
        collector_id: &str,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, DataLayerError> {
        let row = match sqlx::query(
            r#"
UPDATE dimension_collectors
SET
  api_format = $2,
  task_type = $3,
  dimension_name = $4,
  source_type = $5,
  source_path = $6,
  value_type = $7,
  transform_expression = $8,
  default_value = $9,
  priority = $10,
  is_enabled = $11,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
            "#,
        )
        .bind(collector_id)
        .bind(&input.api_format)
        .bind(&input.task_type)
        .bind(&input.dimension_name)
        .bind(&input.source_type)
        .bind(input.source_path.as_deref())
        .bind(&input.value_type)
        .bind(input.transform_expression.as_deref())
        .bind(input.default_value.as_deref())
        .bind(input.priority)
        .bind(input.is_enabled)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(row) => row,
            Err(sqlx::Error::Database(err)) => {
                return Ok(AdminBillingMutationOutcome::Invalid(format!(
                    "Integrity error: {err}"
                )))
            }
            Err(err) => return Err(DataLayerError::postgres(err)),
        };
        match row {
            Some(row) => Ok(AdminBillingMutationOutcome::Applied(
                map_admin_billing_collector_row(&row)?,
            )),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }

    async fn apply_admin_billing_preset(
        &self,
        preset: &str,
        mode: &str,
        collectors: &[AdminBillingCollectorWriteInput],
    ) -> Result<AdminBillingMutationOutcome<AdminBillingPresetApplyResult>, DataLayerError> {
        let mut created = 0_u64;
        let mut updated = 0_u64;
        let mut skipped = 0_u64;
        let mut errors = Vec::new();

        for collector in collectors {
            let existing_id = match sqlx::query_scalar::<_, String>(
                r#"
SELECT id
FROM dimension_collectors
WHERE api_format = $1
  AND task_type = $2
  AND dimension_name = $3
  AND priority = $4
  AND is_enabled = TRUE
LIMIT 1
                "#,
            )
            .bind(&collector.api_format)
            .bind(&collector.task_type)
            .bind(&collector.dimension_name)
            .bind(collector.priority)
            .fetch_optional(&self.pool)
            .await
            {
                Ok(value) => value,
                Err(err) => {
                    errors.push(format!(
                        "Failed to query collector: api_format={} task_type={} dim={}: {}",
                        collector.api_format, collector.task_type, collector.dimension_name, err
                    ));
                    continue;
                }
            };

            if let Some(existing_id) = existing_id {
                if mode == "overwrite" {
                    match sqlx::query(
                        r#"
UPDATE dimension_collectors
SET
  source_type = $2,
  source_path = $3,
  value_type = $4,
  transform_expression = $5,
  default_value = $6,
  is_enabled = $7,
  updated_at = NOW()
WHERE id = $1
                        "#,
                    )
                    .bind(&existing_id)
                    .bind(&collector.source_type)
                    .bind(collector.source_path.as_deref())
                    .bind(&collector.value_type)
                    .bind(collector.transform_expression.as_deref())
                    .bind(collector.default_value.as_deref())
                    .bind(collector.is_enabled)
                    .execute(&self.pool)
                    .await
                    {
                        Ok(_) => updated += 1,
                        Err(err) => errors.push(format!(
                            "Failed to update collector {}: {}",
                            existing_id, err
                        )),
                    }
                } else {
                    skipped += 1;
                }
                continue;
            }

            match sqlx::query(
                r#"
INSERT INTO dimension_collectors (
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(), NOW())
                "#,
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&collector.api_format)
            .bind(&collector.task_type)
            .bind(&collector.dimension_name)
            .bind(&collector.source_type)
            .bind(collector.source_path.as_deref())
            .bind(&collector.value_type)
            .bind(collector.transform_expression.as_deref())
            .bind(collector.default_value.as_deref())
            .bind(collector.priority)
            .bind(collector.is_enabled)
            .execute(&self.pool)
            .await
            {
                Ok(_) => created += 1,
                Err(err) => errors.push(format!(
                    "Failed to create collector: api_format={} task_type={} dim={}: {}",
                    collector.api_format, collector.task_type, collector.dimension_name, err
                )),
            }
        }

        Ok(AdminBillingMutationOutcome::Applied(
            AdminBillingPresetApplyResult {
                preset: preset.to_string(),
                mode: mode.to_string(),
                created,
                updated,
                skipped,
                errors,
            },
        ))
    }

    async fn find_payment_gateway_config(
        &self,
        provider: &str,
    ) -> Result<Option<PaymentGatewayConfigRecord>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  provider, enabled, endpoint_url, callback_base_url, merchant_id,
  merchant_key_encrypted, pay_currency,
  CAST(usd_exchange_rate AS DOUBLE PRECISION) AS usd_exchange_rate,
  CAST(min_recharge_usd AS DOUBLE PRECISION) AS min_recharge_usd,
  channels_json,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM payment_gateway_configs
WHERE provider = $1
LIMIT 1
            "#,
        )
        .bind(provider.trim().to_ascii_lowercase())
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref().map(map_payment_gateway_config_row).transpose()
    }

    async fn upsert_payment_gateway_config(
        &self,
        input: &PaymentGatewayConfigWriteInput,
    ) -> Result<AdminBillingMutationOutcome<PaymentGatewayConfigRecord>, DataLayerError> {
        let provider = input.provider.trim().to_ascii_lowercase();
        let row = sqlx::query(
            r#"
INSERT INTO payment_gateway_configs (
  provider, enabled, endpoint_url, callback_base_url, merchant_id,
  merchant_key_encrypted, pay_currency, usd_exchange_rate, min_recharge_usd,
  channels_json, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW(), NOW())
ON CONFLICT (provider)
DO UPDATE SET
  enabled = EXCLUDED.enabled,
  endpoint_url = EXCLUDED.endpoint_url,
  callback_base_url = EXCLUDED.callback_base_url,
  merchant_id = EXCLUDED.merchant_id,
  merchant_key_encrypted = CASE
    WHEN $11::BOOL THEN payment_gateway_configs.merchant_key_encrypted
    ELSE EXCLUDED.merchant_key_encrypted
  END,
  pay_currency = EXCLUDED.pay_currency,
  usd_exchange_rate = EXCLUDED.usd_exchange_rate,
  min_recharge_usd = EXCLUDED.min_recharge_usd,
  channels_json = EXCLUDED.channels_json,
  updated_at = NOW()
RETURNING
  provider, enabled, endpoint_url, callback_base_url, merchant_id,
  merchant_key_encrypted, pay_currency,
  CAST(usd_exchange_rate AS DOUBLE PRECISION) AS usd_exchange_rate,
  CAST(min_recharge_usd AS DOUBLE PRECISION) AS min_recharge_usd,
  channels_json,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
            "#,
        )
        .bind(&provider)
        .bind(input.enabled)
        .bind(&input.endpoint_url)
        .bind(input.callback_base_url.as_deref())
        .bind(&input.merchant_id)
        .bind(input.merchant_key_encrypted.as_deref())
        .bind(&input.pay_currency)
        .bind(input.usd_exchange_rate)
        .bind(input.min_recharge_usd)
        .bind(&input.channels_json)
        .bind(input.preserve_existing_secret)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(AdminBillingMutationOutcome::Applied(
            map_payment_gateway_config_row(&row)?,
        ))
    }

    async fn list_billing_plans(
        &self,
        include_disabled: bool,
    ) -> Result<Option<Vec<BillingPlanRecord>>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  id, title, description,
  CAST(price_amount AS DOUBLE PRECISION) AS price_amount,
  price_currency, duration_unit, duration_value, enabled, sort_order,
  max_active_per_user, purchase_limit_scope, entitlements_json,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM billing_plans
WHERE ($1::BOOL = TRUE OR enabled = TRUE)
ORDER BY sort_order ASC, price_amount ASC, id ASC
            "#,
        )
        .bind(include_disabled)
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(Some(
            rows.iter()
                .map(map_billing_plan_row)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    async fn find_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<Option<BillingPlanRecord>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  id, title, description,
  CAST(price_amount AS DOUBLE PRECISION) AS price_amount,
  price_currency, duration_unit, duration_value, enabled, sort_order,
  max_active_per_user, purchase_limit_scope, entitlements_json,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM billing_plans
WHERE id = $1
LIMIT 1
            "#,
        )
        .bind(plan_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref().map(map_billing_plan_row).transpose()
    }

    async fn create_billing_plan(
        &self,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let id = uuid::Uuid::new_v4().to_string();
        let row = sqlx::query(BILLING_PLAN_INSERT_RETURNING_SQL)
            .bind(&id)
            .bind(&input.title)
            .bind(input.description.as_deref())
            .bind(input.price_amount)
            .bind(&input.price_currency)
            .bind(&input.duration_unit)
            .bind(input.duration_value)
            .bind(input.enabled)
            .bind(input.sort_order)
            .bind(input.max_active_per_user)
            .bind(&input.purchase_limit_scope)
            .bind(&input.entitlements_json)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(AdminBillingMutationOutcome::Applied(map_billing_plan_row(
            &row,
        )?))
    }

    async fn update_billing_plan(
        &self,
        plan_id: &str,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let row = sqlx::query(BILLING_PLAN_UPDATE_RETURNING_SQL)
            .bind(plan_id)
            .bind(&input.title)
            .bind(input.description.as_deref())
            .bind(input.price_amount)
            .bind(&input.price_currency)
            .bind(&input.duration_unit)
            .bind(input.duration_value)
            .bind(input.enabled)
            .bind(input.sort_order)
            .bind(input.max_active_per_user)
            .bind(&input.purchase_limit_scope)
            .bind(&input.entitlements_json)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        match row {
            Some(row) => Ok(AdminBillingMutationOutcome::Applied(map_billing_plan_row(
                &row,
            )?)),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }

    async fn set_billing_plan_enabled(
        &self,
        plan_id: &str,
        enabled: bool,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let row = sqlx::query(
            r#"
UPDATE billing_plans
SET enabled = $2, updated_at = NOW()
WHERE id = $1
RETURNING
  id, title, description,
  CAST(price_amount AS DOUBLE PRECISION) AS price_amount,
  price_currency, duration_unit, duration_value, enabled, sort_order,
  max_active_per_user, purchase_limit_scope, entitlements_json,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
            "#,
        )
        .bind(plan_id)
        .bind(enabled)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        match row {
            Some(row) => Ok(AdminBillingMutationOutcome::Applied(map_billing_plan_row(
                &row,
            )?)),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }

    async fn delete_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<AdminBillingMutationOutcome<()>, DataLayerError> {
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM billing_plans WHERE id = $1",
        )
        .bind(plan_id)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        if exists == 0 {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }

        let order_count = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)::bigint
FROM payment_orders
WHERE product_id = $1
  AND order_kind = 'plan_purchase'
            "#,
        )
        .bind(plan_id)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        let entitlement_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM user_plan_entitlements WHERE plan_id = $1",
        )
        .bind(plan_id)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        if order_count > 0 || entitlement_count > 0 {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "套餐已有订单或权益，不能删除，请停用该套餐".to_string(),
            ));
        }

        let result = sqlx::query("DELETE FROM billing_plans WHERE id = $1")
            .bind(plan_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        if result.rows_affected() == 0 {
            Ok(AdminBillingMutationOutcome::NotFound)
        } else {
            Ok(AdminBillingMutationOutcome::Applied(()))
        }
    }

    async fn list_user_plan_entitlements(
        &self,
        user_id: &str,
    ) -> Result<Option<Vec<UserPlanEntitlementRecord>>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  id, user_id, plan_id, payment_order_id, status,
  CAST(EXTRACT(EPOCH FROM starts_at) AS BIGINT) AS starts_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  entitlements_snapshot,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM user_plan_entitlements
WHERE user_id = $1
  AND status = 'active'
  AND expires_at > NOW()
ORDER BY expires_at ASC, created_at ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(Some(
            rows.iter()
                .map(map_user_plan_entitlement_row)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    async fn find_user_daily_quota_availability(
        &self,
        user_id: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT id, entitlements_snapshot
FROM user_plan_entitlements
WHERE user_id = $1
  AND status = 'active'
  AND starts_at <= NOW()
  AND expires_at > NOW()
ORDER BY expires_at ASC, created_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        let now = chrono::Utc::now();
        let mut grants = Vec::new();
        for row in rows {
            let entitlement_id: String = row.try_get("id").map_postgres_err()?;
            let entitlements: serde_json::Value =
                row.try_get("entitlements_snapshot").map_postgres_err()?;
            grants.extend(daily_quota_grants_from_entitlement(
                &entitlement_id,
                &entitlements,
                now,
            )?);
        }

        let mut total_quota_usd = 0.0;
        let mut used_usd = 0.0;
        let mut remaining_usd = 0.0;
        let mut allow_wallet_overage = true;
        for grant in &grants {
            allow_wallet_overage &= grant.allow_wallet_overage;
            let used = sqlx::query_scalar::<_, Option<f64>>(
                r#"
SELECT CAST(COALESCE(SUM(amount_usd), 0) AS DOUBLE PRECISION)
FROM entitlement_usage_ledgers
WHERE user_entitlement_id = $1
  AND usage_date = $2
                "#,
            )
            .bind(&grant.entitlement_id)
            .bind(&grant.usage_date)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
            .unwrap_or(0.0);
            total_quota_usd += grant.daily_quota_usd;
            used_usd += used.min(grant.daily_quota_usd).max(0.0);
            remaining_usd += (grant.daily_quota_usd - used).max(0.0);
        }
        let has_active_daily_quota = !grants.is_empty();
        Ok(Some(UserDailyQuotaAvailabilityRecord {
            has_active_daily_quota,
            total_quota_usd,
            used_usd,
            remaining_usd,
            allow_wallet_overage: has_active_daily_quota && allow_wallet_overage,
        }))
    }
}

const BILLING_PLAN_INSERT_RETURNING_SQL: &str = r#"
INSERT INTO billing_plans (
  id, title, description, price_amount, price_currency, duration_unit,
  duration_value, enabled, sort_order, max_active_per_user, purchase_limit_scope,
  entitlements_json, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NOW(), NOW())
RETURNING
  id, title, description,
  CAST(price_amount AS DOUBLE PRECISION) AS price_amount,
  price_currency, duration_unit, duration_value, enabled, sort_order,
  max_active_per_user, purchase_limit_scope, entitlements_json,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
"#;

const BILLING_PLAN_UPDATE_RETURNING_SQL: &str = r#"
UPDATE billing_plans
SET
  title = $2,
  description = $3,
  price_amount = $4,
  price_currency = $5,
  duration_unit = $6,
  duration_value = $7,
  enabled = $8,
  sort_order = $9,
  max_active_per_user = $10,
  purchase_limit_scope = $11,
  entitlements_json = $12,
  updated_at = NOW()
WHERE id = $1
RETURNING
  id, title, description,
  CAST(price_amount AS DOUBLE PRECISION) AS price_amount,
  price_currency, duration_unit, duration_value, enabled, sort_order,
  max_active_per_user, purchase_limit_scope, entitlements_json,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
"#;

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

fn read_count(row: sqlx::postgres::PgRow) -> Result<u64, DataLayerError> {
    Ok(row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64)
}

#[derive(Debug)]
struct DailyQuotaGrant {
    entitlement_id: String,
    daily_quota_usd: f64,
    usage_date: String,
    allow_wallet_overage: bool,
}

fn daily_quota_usage_date(
    reset_timezone: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, DataLayerError> {
    let timezone = reset_timezone
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Asia/Shanghai")
        .parse::<chrono_tz::Tz>()
        .map_err(|err| DataLayerError::InvalidInput(format!("invalid reset_timezone: {err}")))?;
    Ok(now.with_timezone(&timezone).date_naive().to_string())
}

fn daily_quota_grants_from_entitlement(
    entitlement_id: &str,
    entitlements: &serde_json::Value,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<DailyQuotaGrant>, DataLayerError> {
    let mut grants = Vec::new();
    let Some(items) = entitlements.as_array() else {
        return Ok(grants);
    };
    for item in items {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("daily_quota") {
            continue;
        }
        let daily_quota_usd = item
            .get("daily_quota_usd")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        if !daily_quota_usd.is_finite() || daily_quota_usd <= 0.0 {
            continue;
        }
        grants.push(DailyQuotaGrant {
            entitlement_id: entitlement_id.to_string(),
            daily_quota_usd,
            usage_date: daily_quota_usage_date(
                item.get("reset_timezone")
                    .and_then(serde_json::Value::as_str),
                now,
            )?,
            allow_wallet_overage: item
                .get("allow_wallet_overage")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        });
    }
    Ok(grants)
}

fn map_payment_gateway_config_row(
    row: &sqlx::postgres::PgRow,
) -> Result<PaymentGatewayConfigRecord, DataLayerError> {
    Ok(PaymentGatewayConfigRecord {
        provider: row.try_get("provider").map_postgres_err()?,
        enabled: row.try_get("enabled").map_postgres_err()?,
        endpoint_url: row.try_get("endpoint_url").map_postgres_err()?,
        callback_base_url: row.try_get("callback_base_url").map_postgres_err()?,
        merchant_id: row.try_get("merchant_id").map_postgres_err()?,
        merchant_key_encrypted: row.try_get("merchant_key_encrypted").map_postgres_err()?,
        pay_currency: row.try_get("pay_currency").map_postgres_err()?,
        usd_exchange_rate: row.try_get("usd_exchange_rate").map_postgres_err()?,
        min_recharge_usd: row.try_get("min_recharge_usd").map_postgres_err()?,
        channels_json: row
            .try_get::<Option<serde_json::Value>, _>("channels_json")
            .map_postgres_err()?
            .unwrap_or_else(|| serde_json::json!([])),
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn map_billing_plan_row(row: &sqlx::postgres::PgRow) -> Result<BillingPlanRecord, DataLayerError> {
    Ok(BillingPlanRecord {
        id: row.try_get("id").map_postgres_err()?,
        title: row.try_get("title").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        price_amount: row.try_get("price_amount").map_postgres_err()?,
        price_currency: row.try_get("price_currency").map_postgres_err()?,
        duration_unit: row.try_get("duration_unit").map_postgres_err()?,
        duration_value: row.try_get("duration_value").map_postgres_err()?,
        enabled: row.try_get("enabled").map_postgres_err()?,
        sort_order: row.try_get("sort_order").map_postgres_err()?,
        max_active_per_user: row.try_get("max_active_per_user").map_postgres_err()?,
        purchase_limit_scope: row.try_get("purchase_limit_scope").map_postgres_err()?,
        entitlements_json: row.try_get("entitlements_json").map_postgres_err()?,
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn map_user_plan_entitlement_row(
    row: &sqlx::postgres::PgRow,
) -> Result<UserPlanEntitlementRecord, DataLayerError> {
    Ok(UserPlanEntitlementRecord {
        id: row.try_get("id").map_postgres_err()?,
        user_id: row.try_get("user_id").map_postgres_err()?,
        plan_id: row.try_get("plan_id").map_postgres_err()?,
        payment_order_id: row.try_get("payment_order_id").map_postgres_err()?,
        status: row.try_get("status").map_postgres_err()?,
        starts_at_unix_secs: row
            .try_get::<i64, _>("starts_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
        expires_at_unix_secs: row
            .try_get::<i64, _>("expires_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
        entitlements_snapshot: row.try_get("entitlements_snapshot").map_postgres_err()?,
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn map_admin_billing_rule_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminBillingRuleRecord, DataLayerError> {
    Ok(AdminBillingRuleRecord {
        id: row.try_get("id").map_postgres_err()?,
        name: row.try_get("name").map_postgres_err()?,
        task_type: row.try_get("task_type").map_postgres_err()?,
        global_model_id: row.try_get("global_model_id").map_postgres_err()?,
        model_id: row.try_get("model_id").map_postgres_err()?,
        expression: row.try_get("expression").map_postgres_err()?,
        variables: row
            .try_get::<Option<serde_json::Value>, _>("variables")
            .map_postgres_err()?
            .unwrap_or_else(|| serde_json::json!({})),
        dimension_mappings: row
            .try_get::<Option<serde_json::Value>, _>("dimension_mappings")
            .map_postgres_err()?
            .unwrap_or_else(|| serde_json::json!({})),
        is_enabled: row.try_get("is_enabled").map_postgres_err()?,
        created_at_unix_ms: row
            .try_get::<i64, _>("created_at_unix_ms")
            .map_postgres_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn map_admin_billing_collector_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminBillingCollectorRecord, DataLayerError> {
    Ok(AdminBillingCollectorRecord {
        id: row.try_get("id").map_postgres_err()?,
        api_format: row.try_get("api_format").map_postgres_err()?,
        task_type: row.try_get("task_type").map_postgres_err()?,
        dimension_name: row.try_get("dimension_name").map_postgres_err()?,
        source_type: row.try_get("source_type").map_postgres_err()?,
        source_path: row.try_get("source_path").map_postgres_err()?,
        value_type: row.try_get("value_type").map_postgres_err()?,
        transform_expression: row.try_get("transform_expression").map_postgres_err()?,
        default_value: row.try_get("default_value").map_postgres_err()?,
        priority: row.try_get("priority").map_postgres_err()?,
        is_enabled: row.try_get("is_enabled").map_postgres_err()?,
        created_at_unix_ms: row
            .try_get::<i64, _>("created_at_unix_ms")
            .map_postgres_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::SqlxBillingReadRepository;
    use crate::{PostgresPoolConfig, PostgresPoolFactory};

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
