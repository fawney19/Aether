use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, Row};

use super::{
    AdminBillingCollectorRecord, AdminBillingCollectorWriteInput, AdminBillingMutationOutcome,
    AdminBillingPresetApplyResult, AdminBillingRuleRecord, AdminBillingRuleWriteInput,
    BillingPlanRecord, BillingPlanWriteInput, BillingReadRepository, PaymentGatewayConfigRecord,
    PaymentGatewayConfigWriteInput, StoredBillingModelContext, UserDailyQuotaAvailabilityRecord,
    UserPlanEntitlementRecord,
};
use crate::driver::sqlite::{sqlite_optional_real, SqlitePool};
use crate::error::SqlResultExt;
use crate::DataLayerError;

const MODEL_CONTEXT_COLUMNS: &str = r#"
SELECT
  p.id AS provider_id,
  p.billing_type AS provider_billing_type,
  pak.id AS provider_api_key_id,
  pak.rate_multipliers AS provider_api_key_rate_multipliers,
  pak.cache_ttl_minutes AS provider_api_key_cache_ttl_minutes,
  gm.id AS global_model_id,
  gm.name AS global_model_name,
  gm.config AS global_model_config,
  CAST(gm.default_price_per_request AS REAL) AS default_price_per_request,
  gm.default_tiered_pricing AS default_tiered_pricing,
  m.id AS model_id,
  m.provider_model_name AS model_provider_model_name,
  m.config AS model_config,
  CAST(m.price_per_request AS REAL) AS model_price_per_request,
  m.tiered_pricing AS model_tiered_pricing,
  m.provider_model_mappings AS provider_model_mappings,
  m.is_available AS model_is_available,
  m.created_at AS model_created_at
FROM providers p
"#;

#[derive(Debug, Clone)]
pub struct SqliteBillingReadRepository {
    pool: SqlitePool,
}

impl SqliteBillingReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl BillingReadRepository for SqliteBillingReadRepository {
    async fn find_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let rows = sqlx::query(&format!(
            r#"
{MODEL_CONTEXT_COLUMNS}
INNER JOIN global_models gm
  ON gm.is_active = 1
LEFT JOIN models m
  ON m.global_model_id = gm.id
 AND m.provider_id = p.id
 AND m.is_active = 1
LEFT JOIN provider_api_keys pak
  ON pak.id = ?
 AND pak.provider_id = p.id
WHERE p.id = ?
  AND (
    gm.name = ?
    OR m.provider_model_name = ?
    OR m.provider_model_mappings IS NOT NULL
  )
"#
        ))
        .bind(provider_api_key_id)
        .bind(provider_id)
        .bind(global_model_name)
        .bind(global_model_name)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        rows.iter()
            .filter_map(|row| match_rank(row, global_model_name).transpose())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .min_by_key(|candidate| {
                (
                    candidate.rank,
                    !candidate.is_available,
                    candidate.pricing_rank,
                    candidate.created_at,
                )
            })
            .map(|candidate| candidate.context)
            .transpose()
    }

    async fn find_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let row = sqlx::query(&format!(
            r#"
{MODEL_CONTEXT_COLUMNS}
INNER JOIN models m
  ON m.id = ?
 AND m.provider_id = p.id
 AND m.is_active = 1
INNER JOIN global_models gm
  ON gm.id = m.global_model_id
 AND gm.is_active = 1
LEFT JOIN provider_api_keys pak
  ON pak.id = ?
 AND pak.provider_id = p.id
WHERE p.id = ?
LIMIT 1
"#
        ))
        .bind(model_id)
        .bind(provider_api_key_id)
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref().map(map_row).transpose()
    }

    async fn admin_billing_enabled_default_value_exists(
        &self,
        api_format: &str,
        task_type: &str,
        dimension_name: &str,
        existing_id: Option<&str>,
    ) -> Result<Option<bool>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT COUNT(*) AS total
FROM dimension_collectors
WHERE api_format = ?
  AND task_type = ?
  AND dimension_name = ?
  AND is_enabled = 1
  AND default_value IS NOT NULL
  AND (? IS NULL OR id <> ?)
            "#,
        )
        .bind(api_format)
        .bind(task_type)
        .bind(dimension_name)
        .bind(existing_id)
        .bind(existing_id)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        Ok(Some(read_count_sqlite(&row)? > 0))
    }

    async fn create_admin_billing_rule(
        &self,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, DataLayerError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = current_unix_secs_i64();
        let result = sqlx::query(
            r#"
INSERT INTO billing_rules (
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(&input.name)
        .bind(&input.task_type)
        .bind(input.global_model_id.as_deref())
        .bind(input.model_id.as_deref())
        .bind(&input.expression)
        .bind(json_to_string(&input.variables)?)
        .bind(json_to_string(&input.dimension_mappings)?)
        .bind(input.is_enabled)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await;
        if let Err(err) = result {
            return Ok(AdminBillingMutationOutcome::Invalid(format!(
                "Integrity error: {err}"
            )));
        }
        match find_admin_billing_rule_sqlite(&self.pool, &id).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Err(DataLayerError::UnexpectedValue(
                "created billing rule missing".to_string(),
            )),
        }
    }

    async fn list_admin_billing_rules(
        &self,
        task_type: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingRuleRecord>, u64)>, DataLayerError> {
        let total_row = sqlx::query(
            r#"
SELECT COUNT(*) AS total
FROM billing_rules
WHERE (? IS NULL OR task_type = ?)
  AND (? IS NULL OR is_enabled = ?)
            "#,
        )
        .bind(task_type)
        .bind(task_type)
        .bind(is_enabled)
        .bind(is_enabled)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        let total = read_count_sqlite(&total_row)?;
        let offset = u64::from(page.saturating_sub(1) * page_size);
        let rows = sqlx::query(
            r#"
SELECT
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled, created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
FROM billing_rules
WHERE (? IS NULL OR task_type = ?)
  AND (? IS NULL OR is_enabled = ?)
ORDER BY updated_at DESC, id DESC
LIMIT ? OFFSET ?
            "#,
        )
        .bind(task_type)
        .bind(task_type)
        .bind(is_enabled)
        .bind(is_enabled)
        .bind(i64::from(page_size))
        .bind(
            i64::try_from(offset)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_admin_billing_rule_sqlite)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((items, total)))
    }

    async fn find_admin_billing_rule(
        &self,
        rule_id: &str,
    ) -> Result<Option<AdminBillingRuleRecord>, DataLayerError> {
        find_admin_billing_rule_sqlite(&self.pool, rule_id).await
    }

    async fn update_admin_billing_rule(
        &self,
        rule_id: &str,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE billing_rules
SET name = ?,
    task_type = ?,
    global_model_id = ?,
    model_id = ?,
    expression = ?,
    variables = ?,
    dimension_mappings = ?,
    is_enabled = ?,
    updated_at = ?
WHERE id = ?
            "#,
        )
        .bind(&input.name)
        .bind(&input.task_type)
        .bind(input.global_model_id.as_deref())
        .bind(input.model_id.as_deref())
        .bind(&input.expression)
        .bind(json_to_string(&input.variables)?)
        .bind(json_to_string(&input.dimension_mappings)?)
        .bind(input.is_enabled)
        .bind(current_unix_secs_i64())
        .bind(rule_id)
        .execute(&self.pool)
        .await;
        let affected = match result {
            Ok(result) => result.rows_affected(),
            Err(err) => {
                return Ok(AdminBillingMutationOutcome::Invalid(format!(
                    "Integrity error: {err}"
                )))
            }
        };
        if affected == 0 {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }
        match find_admin_billing_rule_sqlite(&self.pool, rule_id).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }

    async fn create_admin_billing_collector(
        &self,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, DataLayerError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = current_unix_secs_i64();
        let result = sqlx::query(
            r#"
INSERT INTO dimension_collectors (
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
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
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await;
        if let Err(err) = result {
            return Ok(AdminBillingMutationOutcome::Invalid(format!(
                "Integrity error: {err}"
            )));
        }
        match find_admin_billing_collector_sqlite(&self.pool, &id).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Err(DataLayerError::UnexpectedValue(
                "created billing collector missing".to_string(),
            )),
        }
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
        let total_row = sqlx::query(
            r#"
SELECT COUNT(*) AS total
FROM dimension_collectors
WHERE (? IS NULL OR api_format = ?)
  AND (? IS NULL OR task_type = ?)
  AND (? IS NULL OR dimension_name = ?)
  AND (? IS NULL OR is_enabled = ?)
            "#,
        )
        .bind(api_format)
        .bind(api_format)
        .bind(task_type)
        .bind(task_type)
        .bind(dimension_name)
        .bind(dimension_name)
        .bind(is_enabled)
        .bind(is_enabled)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        let total = read_count_sqlite(&total_row)?;
        let offset = u64::from(page.saturating_sub(1) * page_size);
        let rows = sqlx::query(
            r#"
SELECT
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled,
  created_at AS created_at_unix_ms, updated_at AS updated_at_unix_secs
FROM dimension_collectors
WHERE (? IS NULL OR api_format = ?)
  AND (? IS NULL OR task_type = ?)
  AND (? IS NULL OR dimension_name = ?)
  AND (? IS NULL OR is_enabled = ?)
ORDER BY updated_at DESC, priority DESC, id ASC
LIMIT ? OFFSET ?
            "#,
        )
        .bind(api_format)
        .bind(api_format)
        .bind(task_type)
        .bind(task_type)
        .bind(dimension_name)
        .bind(dimension_name)
        .bind(is_enabled)
        .bind(is_enabled)
        .bind(i64::from(page_size))
        .bind(
            i64::try_from(offset)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_admin_billing_collector_sqlite)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((items, total)))
    }

    async fn find_admin_billing_collector(
        &self,
        collector_id: &str,
    ) -> Result<Option<AdminBillingCollectorRecord>, DataLayerError> {
        find_admin_billing_collector_sqlite(&self.pool, collector_id).await
    }

    async fn update_admin_billing_collector(
        &self,
        collector_id: &str,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE dimension_collectors
SET api_format = ?,
    task_type = ?,
    dimension_name = ?,
    source_type = ?,
    source_path = ?,
    value_type = ?,
    transform_expression = ?,
    default_value = ?,
    priority = ?,
    is_enabled = ?,
    updated_at = ?
WHERE id = ?
            "#,
        )
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
        .bind(current_unix_secs_i64())
        .bind(collector_id)
        .execute(&self.pool)
        .await;
        let affected = match result {
            Ok(result) => result.rows_affected(),
            Err(err) => {
                return Ok(AdminBillingMutationOutcome::Invalid(format!(
                    "Integrity error: {err}"
                )))
            }
        };
        if affected == 0 {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }
        match find_admin_billing_collector_sqlite(&self.pool, collector_id).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
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
WHERE api_format = ?
  AND task_type = ?
  AND dimension_name = ?
  AND priority = ?
  AND is_enabled = 1
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
SET source_type = ?,
    source_path = ?,
    value_type = ?,
    transform_expression = ?,
    default_value = ?,
    is_enabled = ?,
    updated_at = ?
WHERE id = ?
                        "#,
                    )
                    .bind(&collector.source_type)
                    .bind(collector.source_path.as_deref())
                    .bind(&collector.value_type)
                    .bind(collector.transform_expression.as_deref())
                    .bind(collector.default_value.as_deref())
                    .bind(collector.is_enabled)
                    .bind(current_unix_secs_i64())
                    .bind(&existing_id)
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

            let id = uuid::Uuid::new_v4().to_string();
            let now = current_unix_secs_i64();
            match sqlx::query(
                r#"
INSERT INTO dimension_collectors (
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(id)
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
            .bind(now)
            .bind(now)
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
  merchant_key_encrypted, pay_currency, usd_exchange_rate, min_recharge_usd,
  channels_json, created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM payment_gateway_configs
WHERE provider = ?
LIMIT 1
            "#,
        )
        .bind(provider.trim().to_ascii_lowercase())
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref()
            .map(map_payment_gateway_config_sqlite)
            .transpose()
    }

    async fn upsert_payment_gateway_config(
        &self,
        input: &PaymentGatewayConfigWriteInput,
    ) -> Result<AdminBillingMutationOutcome<PaymentGatewayConfigRecord>, DataLayerError> {
        let provider = input.provider.trim().to_ascii_lowercase();
        let existing_secret = if input.preserve_existing_secret {
            sqlx::query_scalar::<_, String>(
                "SELECT merchant_key_encrypted FROM payment_gateway_configs WHERE provider = ?",
            )
            .bind(&provider)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?
        } else {
            None
        };
        let secret = if input.preserve_existing_secret {
            existing_secret
        } else {
            input.merchant_key_encrypted.clone()
        };
        let now = current_unix_secs_i64();
        sqlx::query(
            r#"
INSERT INTO payment_gateway_configs (
  provider, enabled, endpoint_url, callback_base_url, merchant_id,
  merchant_key_encrypted, pay_currency, usd_exchange_rate, min_recharge_usd,
  channels_json, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(provider) DO UPDATE SET
  enabled = excluded.enabled,
  endpoint_url = excluded.endpoint_url,
  callback_base_url = excluded.callback_base_url,
  merchant_id = excluded.merchant_id,
  merchant_key_encrypted = excluded.merchant_key_encrypted,
  pay_currency = excluded.pay_currency,
  usd_exchange_rate = excluded.usd_exchange_rate,
  min_recharge_usd = excluded.min_recharge_usd,
  channels_json = excluded.channels_json,
  updated_at = excluded.updated_at
            "#,
        )
        .bind(&provider)
        .bind(input.enabled)
        .bind(&input.endpoint_url)
        .bind(input.callback_base_url.as_deref())
        .bind(&input.merchant_id)
        .bind(secret.as_deref())
        .bind(&input.pay_currency)
        .bind(input.usd_exchange_rate)
        .bind(input.min_recharge_usd)
        .bind(json_to_string(&input.channels_json)?)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        match self.find_payment_gateway_config(&provider).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Err(DataLayerError::UnexpectedValue(
                "upserted payment gateway config missing".to_string(),
            )),
        }
    }

    async fn list_billing_plans(
        &self,
        include_disabled: bool,
    ) -> Result<Option<Vec<BillingPlanRecord>>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  id, title, description, price_amount, price_currency, duration_unit,
  duration_value, enabled, sort_order, max_active_per_user, purchase_limit_scope,
  entitlements_json,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM billing_plans
WHERE (? = 1 OR enabled = 1)
ORDER BY sort_order ASC, price_amount ASC, id ASC
            "#,
        )
        .bind(include_disabled)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        Ok(Some(
            rows.iter()
                .map(map_billing_plan_sqlite)
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
  id, title, description, price_amount, price_currency, duration_unit,
  duration_value, enabled, sort_order, max_active_per_user, purchase_limit_scope,
  entitlements_json,
  created_at AS created_at_unix_secs, updated_at AS updated_at_unix_secs
FROM billing_plans
WHERE id = ?
LIMIT 1
            "#,
        )
        .bind(plan_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref().map(map_billing_plan_sqlite).transpose()
    }

    async fn create_billing_plan(
        &self,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = current_unix_secs_i64();
        sqlx::query(BILLING_PLAN_INSERT_SQLITE)
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
            .bind(json_to_string(&input.entitlements_json)?)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        match self.find_billing_plan(&id).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Err(DataLayerError::UnexpectedValue(
                "created billing plan missing".to_string(),
            )),
        }
    }

    async fn update_billing_plan(
        &self,
        plan_id: &str,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let result = sqlx::query(BILLING_PLAN_UPDATE_SQLITE)
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
            .bind(json_to_string(&input.entitlements_json)?)
            .bind(current_unix_secs_i64())
            .bind(plan_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }
        match self.find_billing_plan(plan_id).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }

    async fn set_billing_plan_enabled(
        &self,
        plan_id: &str,
        enabled: bool,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let result =
            sqlx::query("UPDATE billing_plans SET enabled = ?, updated_at = ? WHERE id = ?")
                .bind(enabled)
                .bind(current_unix_secs_i64())
                .bind(plan_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }
        match self.find_billing_plan(plan_id).await? {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }

    async fn delete_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<AdminBillingMutationOutcome<()>, DataLayerError> {
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM billing_plans WHERE id = ?")
                .bind(plan_id)
                .fetch_one(&self.pool)
                .await
                .map_sql_err()?;
        if exists == 0 {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }

        let order_count = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)
FROM payment_orders
WHERE product_id = ?
  AND order_kind = 'plan_purchase'
            "#,
        )
        .bind(plan_id)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        let entitlement_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM user_plan_entitlements WHERE plan_id = ?",
        )
        .bind(plan_id)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        if order_count > 0 || entitlement_count > 0 {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "套餐已有订单或权益，不能删除，请停用该套餐".to_string(),
            ));
        }

        let result = sqlx::query("DELETE FROM billing_plans WHERE id = ?")
            .bind(plan_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
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
  starts_at AS starts_at_unix_secs, expires_at AS expires_at_unix_secs,
  entitlements_snapshot, created_at AS created_at_unix_secs,
  updated_at AS updated_at_unix_secs
FROM user_plan_entitlements
WHERE user_id = ?
  AND status = 'active'
  AND expires_at > ?
ORDER BY expires_at ASC, created_at ASC
            "#,
        )
        .bind(user_id)
        .bind(current_unix_secs_i64())
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        Ok(Some(
            rows.iter()
                .map(map_user_plan_entitlement_sqlite)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    async fn find_user_daily_quota_availability(
        &self,
        user_id: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, DataLayerError> {
        let now_unix_secs = current_unix_secs_i64();
        let rows = sqlx::query(
            r#"
SELECT id, starts_at, entitlements_snapshot
FROM user_plan_entitlements
WHERE user_id = ?
  AND status = 'active'
  AND starts_at <= ?
  AND expires_at > ?
ORDER BY expires_at ASC, created_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let now = chrono::Utc::now();
        let mut grants = Vec::new();
        for row in rows {
            let entitlement_id: String = row.try_get("id").map_sql_err()?;
            let starts_at_unix_secs =
                row.try_get::<i64, _>("starts_at").map_sql_err()?.max(0) as u64;
            let entitlements = parse_json(row.try_get("entitlements_snapshot").ok().flatten())?
                .unwrap_or_else(|| serde_json::json!([]));
            grants.extend(daily_quota_grants_from_entitlement(
                &entitlement_id,
                starts_at_unix_secs,
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
            let mut used = 0.0;
            for usage_date in &grant.usage_dates {
                used += sqlx::query_scalar::<_, f64>(
                    r#"
SELECT CAST(COALESCE(SUM(amount_usd), 0) AS REAL)
FROM entitlement_usage_ledgers
WHERE user_entitlement_id = ?
  AND usage_date = ?
                "#,
                )
                .bind(&grant.entitlement_id)
                .bind(usage_date)
                .fetch_one(&self.pool)
                .await
                .map_sql_err()?;
            }
            total_quota_usd += grant.effective_limit_usd;
            used_usd += used.min(grant.effective_limit_usd).max(0.0);
            remaining_usd += (grant.effective_limit_usd - used).max(0.0);
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

    async fn reset_user_daily_quota(
        &self,
        user_id: &str,
        entitlement_id: &str,
        min_remaining_secs: u64,
        penalty_secs: u64,
    ) -> Result<AdminBillingMutationOutcome<UserPlanEntitlementRecord>, DataLayerError> {
        let min_remaining_secs = i64::try_from(min_remaining_secs)
            .map_err(|_| DataLayerError::InvalidInput("min remaining overflow".to_string()))?;
        let penalty_secs = i64::try_from(penalty_secs)
            .map_err(|_| DataLayerError::InvalidInput("penalty overflow".to_string()))?;
        let now_unix_secs = current_unix_secs_i64();
        let now = now_unix_secs.max(0) as u64;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let row = sqlx::query(
            r#"
SELECT
  id, user_id, plan_id, payment_order_id, status,
  starts_at AS starts_at_unix_secs, expires_at AS expires_at_unix_secs,
  entitlements_snapshot, created_at AS created_at_unix_secs,
  updated_at AS updated_at_unix_secs
FROM user_plan_entitlements
WHERE id = ?
  AND user_id = ?
LIMIT 1
            "#,
        )
        .bind(entitlement_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
        let Some(row) = row else {
            return Ok(AdminBillingMutationOutcome::NotFound);
        };
        let record = map_user_plan_entitlement_sqlite(&row)?;
        if record.status != "active"
            || record.starts_at_unix_secs > now
            || record.expires_at_unix_secs <= now
        {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "权益未生效或已过期".to_string(),
            ));
        }
        if record.expires_at_unix_secs.saturating_sub(now) < min_remaining_secs as u64 {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "剩余有效期不足 72 小时，不能自助重置".to_string(),
            ));
        }
        if !entitlement_has_daily_quota(&record.entitlements_snapshot) {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "权益不包含每日额度".to_string(),
            ));
        }
        if !entitlement_allows_self_service_daily_quota_reset(&record.entitlements_snapshot) {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "该套餐未开放自助重置每日额度".to_string(),
            ));
        }

        let grants = daily_quota_grants_from_entitlement(
            &record.id,
            record.starts_at_unix_secs,
            &record.entitlements_snapshot,
            chrono::Utc::now(),
        )?;
        if grants.is_empty() {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "权益不包含可用每日额度".to_string(),
            ));
        }
        for grant in &grants {
            let current_used = sqlx::query_scalar::<_, f64>(
                r#"
SELECT CAST(COALESCE(SUM(amount_usd), 0) AS REAL)
FROM entitlement_usage_ledgers
WHERE user_entitlement_id = ?
  AND usage_date = ?
                "#,
            )
            .bind(&grant.entitlement_id)
            .bind(&grant.usage_date)
            .fetch_one(&mut *tx)
            .await
            .map_sql_err()?;
            let used_to_reset = current_used.max(0.0);
            if used_to_reset <= 0.000_000_01 {
                continue;
            }
            let balance_before = (grant.effective_limit_usd - current_used).max(0.0);
            let balance_after = (balance_before + used_to_reset).min(grant.effective_limit_usd);
            sqlx::query(
                r#"
INSERT INTO entitlement_usage_ledgers (
  id, user_entitlement_id, user_id, request_id, amount_usd,
  balance_before, balance_after, usage_date, created_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&grant.entitlement_id)
            .bind(user_id)
            .bind(format!(
                "self_service_daily_quota_reset:{}",
                uuid::Uuid::new_v4()
            ))
            .bind(-used_to_reset)
            .bind(balance_before)
            .bind(balance_after)
            .bind(&grant.usage_date)
            .bind(now_unix_secs)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }
        let next_expires_at = (record.expires_at_unix_secs as i64 - penalty_secs).max(0);
        let result = sqlx::query(
            r#"
UPDATE user_plan_entitlements
SET expires_at = ?,
    updated_at = ?
WHERE id = ?
  AND user_id = ?
            "#,
        )
        .bind(next_expires_at)
        .bind(now_unix_secs)
        .bind(entitlement_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }
        let row = sqlx::query(
            r#"
SELECT
  id, user_id, plan_id, payment_order_id, status,
  starts_at AS starts_at_unix_secs, expires_at AS expires_at_unix_secs,
  entitlements_snapshot, created_at AS created_at_unix_secs,
  updated_at AS updated_at_unix_secs
FROM user_plan_entitlements
WHERE id = ?
LIMIT 1
            "#,
        )
        .bind(entitlement_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
        tx.commit().await.map_sql_err()?;
        match row
            .as_ref()
            .map(map_user_plan_entitlement_sqlite)
            .transpose()?
        {
            Some(record) => Ok(AdminBillingMutationOutcome::Applied(record)),
            None => Ok(AdminBillingMutationOutcome::NotFound),
        }
    }
}

const BILLING_PLAN_INSERT_SQLITE: &str = r#"
INSERT INTO billing_plans (
  id, title, description, price_amount, price_currency, duration_unit,
  duration_value, enabled, sort_order, max_active_per_user, purchase_limit_scope,
  entitlements_json,
  created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

const BILLING_PLAN_UPDATE_SQLITE: &str = r#"
UPDATE billing_plans
SET title = ?,
    description = ?,
    price_amount = ?,
    price_currency = ?,
    duration_unit = ?,
    duration_value = ?,
    enabled = ?,
    sort_order = ?,
    max_active_per_user = ?,
    purchase_limit_scope = ?,
    entitlements_json = ?,
    updated_at = ?
WHERE id = ?
"#;

struct RankedContext {
    rank: u8,
    is_available: bool,
    pricing_rank: u8,
    created_at: i64,
    context: Result<StoredBillingModelContext, DataLayerError>,
}

fn match_rank(
    row: &SqliteRow,
    requested_model: &str,
) -> Result<Option<RankedContext>, DataLayerError> {
    let provider_model_name: Option<String> =
        row.try_get("model_provider_model_name").map_sql_err()?;
    let global_model_name: String = row.try_get("global_model_name").map_sql_err()?;
    let mappings: Option<String> = row.try_get("provider_model_mappings").ok().flatten();

    let rank = if provider_model_name.as_deref() == Some(requested_model) {
        0
    } else if mappings
        .as_deref()
        .is_some_and(|mappings| provider_model_mappings_match(mappings, requested_model))
    {
        1
    } else if global_model_name == requested_model {
        2
    } else {
        return Ok(None);
    };

    let has_model_price = sqlite_optional_real(row, "model_price_per_request")?.is_some()
        || row
            .try_get::<Option<String>, _>("model_tiered_pricing")
            .ok()
            .flatten()
            .is_some();
    let has_default_price = sqlite_optional_real(row, "default_price_per_request")?.is_some()
        || row
            .try_get::<Option<String>, _>("default_tiered_pricing")
            .ok()
            .flatten()
            .is_some();
    let pricing_rank = if has_model_price {
        0
    } else if has_default_price {
        1
    } else {
        2
    };

    Ok(Some(RankedContext {
        rank,
        is_available: row
            .try_get::<Option<bool>, _>("model_is_available")
            .map_sql_err()?
            .unwrap_or(false),
        pricing_rank,
        created_at: row
            .try_get::<Option<i64>, _>("model_created_at")
            .map_sql_err()?
            .unwrap_or(i64::MAX),
        context: map_row(row),
    }))
}

fn provider_model_mappings_match(raw: &str, requested_model: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw == requested_model;
    };
    json_mapping_matches(&value, requested_model)
}

fn json_mapping_matches(value: &serde_json::Value, requested_model: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value == requested_model,
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| json_mapping_matches(value, requested_model)),
        serde_json::Value::Object(map) => map
            .get("name")
            .is_some_and(|value| json_mapping_matches(value, requested_model)),
        _ => false,
    }
}

fn map_row(row: &SqliteRow) -> Result<StoredBillingModelContext, DataLayerError> {
    StoredBillingModelContext::new(
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("provider_billing_type").map_sql_err()?,
        row.try_get("provider_api_key_id").map_sql_err()?,
        parse_json(
            row.try_get("provider_api_key_rate_multipliers")
                .ok()
                .flatten(),
        )?,
        row.try_get::<Option<i64>, _>("provider_api_key_cache_ttl_minutes")
            .map_sql_err()?,
        row.try_get("global_model_id").map_sql_err()?,
        row.try_get("global_model_name").map_sql_err()?,
        parse_json(row.try_get("global_model_config").ok().flatten())?,
        sqlite_optional_real(row, "default_price_per_request")?,
        parse_json(row.try_get("default_tiered_pricing").ok().flatten())?,
        row.try_get("model_id").map_sql_err()?,
        row.try_get("model_provider_model_name").map_sql_err()?,
        parse_json(row.try_get("model_config").ok().flatten())?,
        sqlite_optional_real(row, "model_price_per_request")?,
        parse_json(row.try_get("model_tiered_pricing").ok().flatten())?,
    )
}

fn parse_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!("billing JSON field is invalid: {err}"))
            })
        })
        .transpose()
}

fn current_unix_secs_i64() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn json_to_string(value: &serde_json::Value) -> Result<String, DataLayerError> {
    serde_json::to_string(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("billing JSON encode failed: {err}"))
    })
}

#[derive(Debug)]
struct DailyQuotaGrant {
    entitlement_id: String,
    usage_date: String,
    usage_dates: Vec<String>,
    allow_wallet_overage: bool,
    effective_limit_usd: f64,
}

fn daily_quota_usage_dates(
    reset_timezone: Option<&str>,
    starts_at_unix_secs: u64,
    now: chrono::DateTime<chrono::Utc>,
    carry_over_days: u64,
) -> Result<Vec<String>, DataLayerError> {
    let timezone = reset_timezone
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Asia/Shanghai")
        .parse::<chrono_tz::Tz>()
        .map_err(|err| DataLayerError::InvalidInput(format!("invalid reset_timezone: {err}")))?;
    let current_date = now.with_timezone(&timezone).date_naive();
    let starts_at = chrono::DateTime::from_timestamp(starts_at_unix_secs as i64, 0)
        .unwrap_or(now)
        .with_timezone(&timezone)
        .date_naive();
    let first_date = (current_date - chrono::Duration::days(carry_over_days as i64)).max(starts_at);
    let mut dates = Vec::new();
    let mut date = first_date;
    while date <= current_date {
        dates.push(date.to_string());
        date = date
            .succ_opt()
            .ok_or_else(|| DataLayerError::InvalidInput("daily quota date overflow".to_string()))?;
    }
    Ok(dates)
}

fn daily_quota_grants_from_entitlement(
    entitlement_id: &str,
    starts_at_unix_secs: u64,
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
        let carry_over = item
            .get("carry_over")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let carry_over_days = if carry_over {
            item.get("carry_over_days")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 30)
        } else {
            0
        };
        let usage_dates = daily_quota_usage_dates(
            item.get("reset_timezone")
                .and_then(serde_json::Value::as_str),
            starts_at_unix_secs,
            now,
            carry_over_days,
        )?;
        let usage_date = usage_dates
            .last()
            .cloned()
            .unwrap_or_else(|| now.date_naive().to_string());
        let window_limit_usd = daily_quota_usd * usage_dates.len() as f64;
        let multiplier_limit_usd = daily_quota_usd
            * item
                .get("carry_over_limit_multiplier")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(usage_dates.len() as f64)
                .clamp(1.0, usage_dates.len() as f64);
        grants.push(DailyQuotaGrant {
            entitlement_id: entitlement_id.to_string(),
            usage_date,
            usage_dates,
            allow_wallet_overage: item
                .get("allow_wallet_overage")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            effective_limit_usd: window_limit_usd.min(multiplier_limit_usd),
        });
    }
    Ok(grants)
}

fn entitlement_has_daily_quota(entitlements: &serde_json::Value) -> bool {
    entitlements.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("daily_quota"))
    })
}

fn entitlement_allows_self_service_daily_quota_reset(entitlements: &serde_json::Value) -> bool {
    entitlements.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("daily_quota")
                && item
                    .get("self_service_daily_quota_reset")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
        })
    })
}

fn read_count_sqlite(row: &SqliteRow) -> Result<u64, DataLayerError> {
    Ok(row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64)
}

fn map_payment_gateway_config_sqlite(
    row: &SqliteRow,
) -> Result<PaymentGatewayConfigRecord, DataLayerError> {
    Ok(PaymentGatewayConfigRecord {
        provider: row.try_get("provider").map_sql_err()?,
        enabled: row.try_get("enabled").map_sql_err()?,
        endpoint_url: row.try_get("endpoint_url").map_sql_err()?,
        callback_base_url: row.try_get("callback_base_url").map_sql_err()?,
        merchant_id: row.try_get("merchant_id").map_sql_err()?,
        merchant_key_encrypted: row.try_get("merchant_key_encrypted").map_sql_err()?,
        pay_currency: row.try_get("pay_currency").map_sql_err()?,
        usd_exchange_rate: sqlite_optional_real(row, "usd_exchange_rate")?.unwrap_or(0.0),
        min_recharge_usd: sqlite_optional_real(row, "min_recharge_usd")?.unwrap_or(0.0),
        channels_json: parse_json(row.try_get("channels_json").ok().flatten())?
            .unwrap_or_else(|| serde_json::json!([])),
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
    })
}

fn map_billing_plan_sqlite(row: &SqliteRow) -> Result<BillingPlanRecord, DataLayerError> {
    Ok(BillingPlanRecord {
        id: row.try_get("id").map_sql_err()?,
        title: row.try_get("title").map_sql_err()?,
        description: row.try_get("description").map_sql_err()?,
        price_amount: sqlite_optional_real(row, "price_amount")?.unwrap_or(0.0),
        price_currency: row.try_get("price_currency").map_sql_err()?,
        duration_unit: row.try_get("duration_unit").map_sql_err()?,
        duration_value: row.try_get("duration_value").map_sql_err()?,
        enabled: row.try_get("enabled").map_sql_err()?,
        sort_order: row.try_get("sort_order").map_sql_err()?,
        max_active_per_user: row.try_get("max_active_per_user").map_sql_err()?,
        purchase_limit_scope: row
            .try_get::<Option<String>, _>("purchase_limit_scope")
            .map_sql_err()?
            .unwrap_or_else(|| "active_period".to_string()),
        entitlements_json: parse_json(row.try_get("entitlements_json").ok().flatten())?
            .unwrap_or_else(|| serde_json::json!([])),
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
    })
}

fn map_user_plan_entitlement_sqlite(
    row: &SqliteRow,
) -> Result<UserPlanEntitlementRecord, DataLayerError> {
    Ok(UserPlanEntitlementRecord {
        id: row.try_get("id").map_sql_err()?,
        user_id: row.try_get("user_id").map_sql_err()?,
        plan_id: row.try_get("plan_id").map_sql_err()?,
        payment_order_id: row.try_get("payment_order_id").map_sql_err()?,
        status: row.try_get("status").map_sql_err()?,
        starts_at_unix_secs: row
            .try_get::<i64, _>("starts_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
        expires_at_unix_secs: row
            .try_get::<i64, _>("expires_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
        entitlements_snapshot: parse_json(row.try_get("entitlements_snapshot").ok().flatten())?
            .unwrap_or_else(|| serde_json::json!([])),
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
    })
}

async fn find_admin_billing_rule_sqlite(
    pool: &SqlitePool,
    rule_id: &str,
) -> Result<Option<AdminBillingRuleRecord>, DataLayerError> {
    let row = sqlx::query(
        r#"
SELECT
  id, name, task_type, global_model_id, model_id, expression, variables,
  dimension_mappings, is_enabled, created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
FROM billing_rules
WHERE id = ?
        "#,
    )
    .bind(rule_id)
    .fetch_optional(pool)
    .await
    .map_sql_err()?;
    row.as_ref().map(map_admin_billing_rule_sqlite).transpose()
}

fn map_admin_billing_rule_sqlite(
    row: &SqliteRow,
) -> Result<AdminBillingRuleRecord, DataLayerError> {
    Ok(AdminBillingRuleRecord {
        id: row.try_get("id").map_sql_err()?,
        name: row.try_get("name").map_sql_err()?,
        task_type: row.try_get("task_type").map_sql_err()?,
        global_model_id: row.try_get("global_model_id").map_sql_err()?,
        model_id: row.try_get("model_id").map_sql_err()?,
        expression: row.try_get("expression").map_sql_err()?,
        variables: parse_required_json(row.try_get("variables").map_sql_err()?)?,
        dimension_mappings: parse_required_json(row.try_get("dimension_mappings").map_sql_err()?)?,
        is_enabled: row.try_get("is_enabled").map_sql_err()?,
        created_at_unix_ms: row
            .try_get::<i64, _>("created_at_unix_ms")
            .map_sql_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
    })
}

async fn find_admin_billing_collector_sqlite(
    pool: &SqlitePool,
    collector_id: &str,
) -> Result<Option<AdminBillingCollectorRecord>, DataLayerError> {
    let row = sqlx::query(
        r#"
SELECT
  id, api_format, task_type, dimension_name, source_type, source_path, value_type,
  transform_expression, default_value, priority, is_enabled,
  created_at AS created_at_unix_ms, updated_at AS updated_at_unix_secs
FROM dimension_collectors
WHERE id = ?
        "#,
    )
    .bind(collector_id)
    .fetch_optional(pool)
    .await
    .map_sql_err()?;
    row.as_ref()
        .map(map_admin_billing_collector_sqlite)
        .transpose()
}

fn map_admin_billing_collector_sqlite(
    row: &SqliteRow,
) -> Result<AdminBillingCollectorRecord, DataLayerError> {
    Ok(AdminBillingCollectorRecord {
        id: row.try_get("id").map_sql_err()?,
        api_format: row.try_get("api_format").map_sql_err()?,
        task_type: row.try_get("task_type").map_sql_err()?,
        dimension_name: row.try_get("dimension_name").map_sql_err()?,
        source_type: row.try_get("source_type").map_sql_err()?,
        source_path: row.try_get("source_path").map_sql_err()?,
        value_type: row.try_get("value_type").map_sql_err()?,
        transform_expression: row.try_get("transform_expression").map_sql_err()?,
        default_value: row.try_get("default_value").map_sql_err()?,
        priority: row.try_get("priority").map_sql_err()?,
        is_enabled: row.try_get("is_enabled").map_sql_err()?,
        created_at_unix_ms: row
            .try_get::<i64, _>("created_at_unix_ms")
            .map_sql_err()?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_sql_err()?
            .max(0) as u64,
    })
}

fn parse_required_json(raw: String) -> Result<serde_json::Value, DataLayerError> {
    serde_json::from_str(&raw).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("billing JSON field is invalid: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{daily_quota_grants_from_entitlement, SqliteBillingReadRepository};
    use crate::lifecycle::migrate::run_sqlite_migrations;
    use crate::repository::billing::{
        AdminBillingCollectorWriteInput, AdminBillingMutationOutcome, AdminBillingRuleWriteInput,
        BillingPlanWriteInput, BillingReadRepository,
    };

    #[test]
    fn daily_quota_grants_apply_carry_over_window_and_multiplier() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-29T04:00:00Z")
            .expect("now should parse")
            .with_timezone(&chrono::Utc);
        let starts_at_unix_secs = chrono::DateTime::parse_from_rfc3339("2026-05-27T00:00:00+08:00")
            .expect("starts_at should parse")
            .timestamp()
            .max(0) as u64;
        let entitlements = json!([{
            "type": "daily_quota",
            "daily_quota_usd": 10.0,
            "reset_timezone": "Asia/Shanghai",
            "carry_over": true,
            "carry_over_days": 5,
            "carry_over_limit_multiplier": 2.5,
            "allow_wallet_overage": false
        }]);

        let grants = daily_quota_grants_from_entitlement(
            "entitlement-carry",
            starts_at_unix_secs,
            &entitlements,
            now,
        )
        .expect("daily quota grant should build");

        assert_eq!(grants.len(), 1);
        assert_eq!(
            grants[0].usage_dates,
            vec!["2026-05-27", "2026-05-28", "2026-05-29"]
        );
        assert_eq!(grants[0].effective_limit_usd, 25.0);
    }

    #[tokio::test]
    async fn sqlite_repository_reads_billing_model_context() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_billing_context(&pool).await;

        let repository = SqliteBillingReadRepository::new(pool);
        let by_alias = repository
            .find_model_context("provider-1", Some("key-1"), "gpt-upstream-alias")
            .await
            .expect("context lookup should run")
            .expect("context should exist");
        assert_eq!(by_alias.model_id.as_deref(), Some("model-1"));
        assert_eq!(by_alias.provider_api_key_cache_ttl_minutes, Some(60));
        assert_eq!(by_alias.model_price_per_request, Some(0.01));

        let by_model_id = repository
            .find_model_context_by_model_id("provider-1", Some("key-1"), "model-1")
            .await
            .expect("model lookup should run")
            .expect("context should exist");
        assert_eq!(by_model_id.global_model_name, "gpt-5");
    }

    #[tokio::test]
    async fn sqlite_repository_manages_admin_billing_rules_and_collectors() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        let repository = SqliteBillingReadRepository::new(pool);

        let rule = match repository
            .create_admin_billing_rule(&AdminBillingRuleWriteInput {
                name: "Chat rule".to_string(),
                task_type: "chat".to_string(),
                global_model_id: Some("global-1".to_string()),
                model_id: None,
                expression: "total_tokens * 0.01".to_string(),
                variables: json!({"rate": 0.01}),
                dimension_mappings: json!({"tokens": "total_tokens"}),
                is_enabled: true,
            })
            .await
            .expect("rule create should run")
        {
            AdminBillingMutationOutcome::Applied(rule) => rule,
            other => panic!("unexpected rule create outcome: {other:?}"),
        };
        assert_eq!(rule.variables["rate"], json!(0.01));
        let (rules, total) = repository
            .list_admin_billing_rules(Some("chat"), Some(true), 1, 20)
            .await
            .expect("rule list should run")
            .expect("rule list should be available");
        assert_eq!(total, 1);
        assert_eq!(rules[0].id, rule.id);

        let updated_rule = match repository
            .update_admin_billing_rule(
                &rule.id,
                &AdminBillingRuleWriteInput {
                    name: "Updated chat rule".to_string(),
                    task_type: "chat".to_string(),
                    global_model_id: Some("global-1".to_string()),
                    model_id: None,
                    expression: "total_tokens * 0.02".to_string(),
                    variables: json!({"rate": 0.02}),
                    dimension_mappings: json!({"tokens": "total_tokens"}),
                    is_enabled: false,
                },
            )
            .await
            .expect("rule update should run")
        {
            AdminBillingMutationOutcome::Applied(rule) => rule,
            other => panic!("unexpected rule update outcome: {other:?}"),
        };
        assert_eq!(updated_rule.name, "Updated chat rule");
        assert!(!updated_rule.is_enabled);

        let collector = match repository
            .create_admin_billing_collector(&AdminBillingCollectorWriteInput {
                api_format: "openai".to_string(),
                task_type: "chat".to_string(),
                dimension_name: "total_tokens".to_string(),
                source_type: "usage".to_string(),
                source_path: Some("$.usage.total_tokens".to_string()),
                value_type: "float".to_string(),
                transform_expression: None,
                default_value: Some("1".to_string()),
                priority: 10,
                is_enabled: true,
            })
            .await
            .expect("collector create should run")
        {
            AdminBillingMutationOutcome::Applied(collector) => collector,
            other => panic!("unexpected collector create outcome: {other:?}"),
        };
        assert!(repository
            .admin_billing_enabled_default_value_exists("openai", "chat", "total_tokens", None,)
            .await
            .expect("default value check should run")
            .expect("default value check should be available"));
        let (collectors, total) = repository
            .list_admin_billing_collectors(Some("openai"), Some("chat"), None, Some(true), 1, 20)
            .await
            .expect("collector list should run")
            .expect("collector list should be available");
        assert_eq!(total, 1);
        assert_eq!(collectors[0].id, collector.id);

        let preset = match repository
            .apply_admin_billing_preset(
                "openai-chat",
                "overwrite",
                &[AdminBillingCollectorWriteInput {
                    api_format: "openai".to_string(),
                    task_type: "chat".to_string(),
                    dimension_name: "total_tokens".to_string(),
                    source_type: "usage".to_string(),
                    source_path: Some("$.usage.total_tokens".to_string()),
                    value_type: "float".to_string(),
                    transform_expression: Some("max(total_tokens, 1)".to_string()),
                    default_value: Some("1".to_string()),
                    priority: 10,
                    is_enabled: true,
                }],
            )
            .await
            .expect("preset apply should run")
        {
            AdminBillingMutationOutcome::Applied(result) => result,
            other => panic!("unexpected preset outcome: {other:?}"),
        };
        assert_eq!(preset.updated, 1);
        assert_eq!(preset.errors, Vec::<String>::new());
    }

    #[tokio::test]
    async fn sqlite_repository_deletes_unused_billing_plans_only() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        let repository = SqliteBillingReadRepository::new(pool.clone());

        let input = BillingPlanWriteInput {
            title: "Daily Plan".to_string(),
            description: None,
            price_amount: 100.0,
            price_currency: "CNY".to_string(),
            duration_unit: "month".to_string(),
            duration_value: 1,
            enabled: true,
            sort_order: 10,
            max_active_per_user: 1,
            purchase_limit_scope: "active_period".to_string(),
            entitlements_json: json!([{
                "type": "daily_quota",
                "daily_quota_usd": 50.0,
                "reset_timezone": "Asia/Shanghai",
                "allow_wallet_overage": false
            }]),
        };
        let removable = match repository
            .create_billing_plan(&input)
            .await
            .expect("plan create should run")
        {
            AdminBillingMutationOutcome::Applied(plan) => plan,
            other => panic!("unexpected plan create outcome: {other:?}"),
        };
        assert_eq!(
            repository
                .delete_billing_plan(&removable.id)
                .await
                .expect("plan delete should run"),
            AdminBillingMutationOutcome::Applied(())
        );
        assert!(repository
            .find_billing_plan(&removable.id)
            .await
            .expect("plan lookup should run")
            .is_none());

        let referenced = match repository
            .create_billing_plan(&input)
            .await
            .expect("plan create should run")
        {
            AdminBillingMutationOutcome::Applied(plan) => plan,
            other => panic!("unexpected plan create outcome: {other:?}"),
        };
        sqlx::query(
            r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, amount_usd, payment_method, order_kind,
  product_id, fulfillment_status, status, created_at
)
VALUES ('order-1', 'order-no-1', 'wallet-1', 0, 'epay', 'plan_purchase',
        ?, 'pending', 'pending', 1)
            "#,
        )
        .bind(&referenced.id)
        .execute(&pool)
        .await
        .expect("payment order should seed");
        match repository
            .delete_billing_plan(&referenced.id)
            .await
            .expect("plan delete should run")
        {
            AdminBillingMutationOutcome::Invalid(detail) => {
                assert!(detail.contains("不能删除"));
            }
            other => panic!("unexpected referenced plan delete outcome: {other:?}"),
        }
        assert!(repository
            .find_billing_plan(&referenced.id)
            .await
            .expect("plan lookup should run")
            .is_some());
    }

    #[tokio::test]
    async fn sqlite_repository_resets_self_service_daily_quota_usage() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let now = chrono::Utc::now();
        let now_unix_secs = now.timestamp().max(0);
        let today = now.date_naive().to_string();
        let starts_at = (now - chrono::Duration::hours(2)).timestamp().max(0);
        let expires_at = (now + chrono::Duration::hours(100)).timestamp().max(0);

        sqlx::query(
            r#"
INSERT INTO users (id, username, email, auth_source, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("user-1")
        .bind("user-1")
        .bind("user-1@example.com")
        .bind("local")
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .execute(&pool)
        .await
        .expect("user should seed");

        sqlx::query(
            r#"
INSERT INTO billing_plans (
  id, title, description, price_amount, price_currency, duration_unit,
  duration_value, enabled, sort_order, max_active_per_user, purchase_limit_scope,
  entitlements_json, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("plan-1")
        .bind("Daily")
        .bind(Option::<String>::None)
        .bind(10.0)
        .bind("CNY")
        .bind("month")
        .bind(1_i64)
        .bind(true)
        .bind(0_i64)
        .bind(1_i64)
        .bind("active_period")
        .bind(
            json!([{
                "type": "daily_quota",
                "daily_quota_usd": 10.0,
                "reset_timezone": "UTC",
                "self_service_daily_quota_reset": true
            }])
            .to_string(),
        )
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .execute(&pool)
        .await
        .expect("plan should seed");

        sqlx::query(
            r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, payment_method, order_kind,
  product_id, fulfillment_status, status, created_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("order-1")
        .bind("order-no-1")
        .bind("wallet-1")
        .bind("user-1")
        .bind(10.0)
        .bind("epay")
        .bind("plan_purchase")
        .bind("plan-1")
        .bind("fulfilled")
        .bind("paid")
        .bind(now_unix_secs)
        .execute(&pool)
        .await
        .expect("payment order should seed");

        sqlx::query(
            r#"
INSERT INTO user_plan_entitlements (
  id, user_id, plan_id, payment_order_id, status, starts_at, expires_at,
  entitlements_snapshot, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("entitlement-1")
        .bind("user-1")
        .bind("plan-1")
        .bind("order-1")
        .bind("active")
        .bind(starts_at)
        .bind(expires_at)
        .bind(
            json!([{
                "type": "daily_quota",
                "daily_quota_usd": 10.0,
                "reset_timezone": "UTC",
                "self_service_daily_quota_reset": true
            }])
            .to_string(),
        )
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .execute(&pool)
        .await
        .expect("entitlement should seed");

        sqlx::query(
            r#"
INSERT INTO entitlement_usage_ledgers (
  id, user_entitlement_id, user_id, request_id, amount_usd,
  balance_before, balance_after, usage_date, created_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind("ledger-1")
        .bind("entitlement-1")
        .bind("user-1")
        .bind("usage-1")
        .bind(4.0)
        .bind(10.0)
        .bind(6.0)
        .bind(&today)
        .bind(now_unix_secs)
        .execute(&pool)
        .await
        .expect("usage ledger should seed");

        let repository = SqliteBillingReadRepository::new(pool.clone());
        let result = repository
            .reset_user_daily_quota("user-1", "entitlement-1", 72 * 60 * 60, 24 * 60 * 60)
            .await
            .expect("reset should run");
        let updated = match result {
            AdminBillingMutationOutcome::Applied(record) => record,
            other => panic!("unexpected reset outcome: {other:?}"),
        };
        assert_eq!(
            updated.expires_at_unix_secs,
            (expires_at as u64).saturating_sub(24 * 60 * 60)
        );

        let used = sqlx::query_scalar::<_, f64>(
            r#"
SELECT CAST(COALESCE(SUM(amount_usd), 0) AS REAL)
FROM entitlement_usage_ledgers
WHERE user_entitlement_id = ?
  AND usage_date = ?
            "#,
        )
        .bind("entitlement-1")
        .bind(&today)
        .fetch_one(&pool)
        .await
        .expect("usage sum should query");
        assert!((used - 0.0).abs() < 0.000_001);
    }

    async fn seed_billing_context(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO providers (id, name, provider_type, billing_type, is_active, created_at, updated_at)
VALUES ('provider-1', 'Provider One', 'openai', 'pay_as_you_go', 1, 1, 1)
"#,
        )
        .execute(pool)
        .await
        .expect("provider should seed");
        sqlx::query(
            r#"
INSERT INTO provider_api_keys (
  id, provider_id, name, rate_multipliers, cache_ttl_minutes, created_at, updated_at
)
VALUES ('key-1', 'provider-1', 'Primary', '{"openai:chat":0.8}', 60, 1, 1)
"#,
        )
        .execute(pool)
        .await
        .expect("provider key should seed");
        sqlx::query(
            r#"
INSERT INTO global_models (
  id, name, display_name, is_active, default_price_per_request, default_tiered_pricing,
  config, created_at, updated_at
)
VALUES (
  'global-1', 'gpt-5', 'GPT-5', 1, 0.02,
  '{"tiers":[{"up_to":null,"input_price_per_1m":3.0}]}',
  '{"streaming":true}', 1, 1
)
"#,
        )
        .execute(pool)
        .await
        .expect("global model should seed");
        sqlx::query(
            r#"
INSERT INTO models (
  id, provider_id, global_model_id, provider_model_name, is_active, is_available,
  price_per_request, provider_model_mappings, created_at, updated_at
)
VALUES (
  'model-1', 'provider-1', 'global-1', 'gpt-upstream', 1, 1, 0.01,
  '[{"name":"gpt-upstream-alias"}]', 1, 1
)
"#,
        )
        .execute(pool)
        .await
        .expect("model should seed");
    }
}
