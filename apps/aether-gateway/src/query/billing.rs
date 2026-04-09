use crate::state::AdminBillingPresetApplyResult;
use crate::{
    AdminBillingCollectorRecord, AdminBillingCollectorWriteInput, AdminBillingRuleRecord,
    AdminBillingRuleWriteInput, GatewayError, LocalMutationOutcome,
};
use aether_data::postgres::PostgresPool;
use sqlx::Row;

fn internal(err: impl ToString) -> GatewayError {
    GatewayError::Internal(err.to_string())
}

pub(crate) async fn admin_billing_enabled_default_value_exists(
    pool: &PostgresPool,
    api_format: &str,
    task_type: &str,
    dimension_name: &str,
    existing_id: Option<&str>,
) -> Result<bool, GatewayError> {
    sqlx::query_scalar::<_, bool>(
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
    .fetch_one(pool)
    .await
    .map_err(internal)
}

pub(crate) async fn create_admin_billing_rule(
    pool: &PostgresPool,
    input: &AdminBillingRuleWriteInput,
) -> Result<LocalMutationOutcome<AdminBillingRuleRecord>, GatewayError> {
    let rule_id = uuid::Uuid::new_v4().to_string();
    let row = match sqlx::query(
        r#"
INSERT INTO billing_rules (
  id,
  name,
  task_type,
  global_model_id,
  model_id,
  expression,
  variables,
  dimension_mappings,
  is_enabled,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  NOW(),
  NOW()
)
RETURNING
  id,
  name,
  task_type,
  global_model_id,
  model_id,
  expression,
  variables,
  dimension_mappings,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
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
    .fetch_one(pool)
    .await
    {
        Ok(row) => row,
        Err(sqlx::Error::Database(err)) => {
            return Ok(LocalMutationOutcome::Invalid(format!(
                "Integrity error: {err}"
            )))
        }
        Err(err) => return Err(GatewayError::Internal(err.to_string())),
    };
    Ok(LocalMutationOutcome::Applied(admin_billing_rule_from_row(
        &row,
    )?))
}

pub(crate) async fn list_admin_billing_rules(
    pool: &PostgresPool,
    task_type: Option<&str>,
    is_enabled: Option<bool>,
    page: u32,
    page_size: u32,
) -> Result<(Vec<AdminBillingRuleRecord>, u64), GatewayError> {
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
        .fetch_one(pool)
        .await
        .map_err(internal)?,
    )?;

    let offset = u64::from(page.saturating_sub(1) * page_size);
    let rows = sqlx::query(
        r#"
SELECT
  id,
  name,
  task_type,
  global_model_id,
  model_id,
  expression,
  variables,
  dimension_mappings,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
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
    .bind(i64::try_from(offset).map_err(|err| GatewayError::Internal(err.to_string()))?)
    .bind(i64::from(page_size))
    .fetch_all(pool)
    .await
    .map_err(internal)?;

    Ok((
        rows.iter()
            .map(admin_billing_rule_from_row)
            .collect::<Result<Vec<_>, _>>()?,
        total,
    ))
}

pub(crate) async fn find_admin_billing_rule(
    pool: &PostgresPool,
    rule_id: &str,
) -> Result<Option<AdminBillingRuleRecord>, GatewayError> {
    let row = sqlx::query(
        r#"
SELECT
  id,
  name,
  task_type,
  global_model_id,
  model_id,
  expression,
  variables,
  dimension_mappings,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM billing_rules
WHERE id = $1
        "#,
    )
    .bind(rule_id)
    .fetch_optional(pool)
    .await
    .map_err(internal)?;
    row.as_ref().map(admin_billing_rule_from_row).transpose()
}

pub(crate) async fn update_admin_billing_rule(
    pool: &PostgresPool,
    rule_id: &str,
    input: &AdminBillingRuleWriteInput,
) -> Result<LocalMutationOutcome<AdminBillingRuleRecord>, GatewayError> {
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
  id,
  name,
  task_type,
  global_model_id,
  model_id,
  expression,
  variables,
  dimension_mappings,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
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
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(sqlx::Error::Database(err)) => {
            return Ok(LocalMutationOutcome::Invalid(format!(
                "Integrity error: {err}"
            )))
        }
        Err(err) => return Err(GatewayError::Internal(err.to_string())),
    };
    match row {
        Some(row) => Ok(LocalMutationOutcome::Applied(admin_billing_rule_from_row(
            &row,
        )?)),
        None => Ok(LocalMutationOutcome::NotFound),
    }
}

pub(crate) async fn create_admin_billing_collector(
    pool: &PostgresPool,
    input: &AdminBillingCollectorWriteInput,
) -> Result<LocalMutationOutcome<AdminBillingCollectorRecord>, GatewayError> {
    let collector_id = uuid::Uuid::new_v4().to_string();
    let row = match sqlx::query(
        r#"
INSERT INTO dimension_collectors (
  id,
  api_format,
  task_type,
  dimension_name,
  source_type,
  source_path,
  value_type,
  transform_expression,
  default_value,
  priority,
  is_enabled,
  created_at,
  updated_at
)
VALUES (
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
  NOW(),
  NOW()
)
RETURNING
  id,
  api_format,
  task_type,
  dimension_name,
  source_type,
  source_path,
  value_type,
  transform_expression,
  default_value,
  priority,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
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
    .fetch_one(pool)
    .await
    {
        Ok(row) => row,
        Err(sqlx::Error::Database(err)) => {
            return Ok(LocalMutationOutcome::Invalid(format!(
                "Integrity error: {err}"
            )))
        }
        Err(err) => return Err(GatewayError::Internal(err.to_string())),
    };
    Ok(LocalMutationOutcome::Applied(
        admin_billing_collector_from_row(&row)?,
    ))
}

pub(crate) async fn list_admin_billing_collectors(
    pool: &PostgresPool,
    api_format: Option<&str>,
    task_type: Option<&str>,
    dimension_name: Option<&str>,
    is_enabled: Option<bool>,
    page: u32,
    page_size: u32,
) -> Result<(Vec<AdminBillingCollectorRecord>, u64), GatewayError> {
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
        .fetch_one(pool)
        .await
        .map_err(internal)?,
    )?;

    let offset = u64::from(page.saturating_sub(1) * page_size);
    let rows = sqlx::query(
        r#"
SELECT
  id,
  api_format,
  task_type,
  dimension_name,
  source_type,
  source_path,
  value_type,
  transform_expression,
  default_value,
  priority,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
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
    .bind(i64::try_from(offset).map_err(|err| GatewayError::Internal(err.to_string()))?)
    .bind(i64::from(page_size))
    .fetch_all(pool)
    .await
    .map_err(internal)?;

    Ok((
        rows.iter()
            .map(admin_billing_collector_from_row)
            .collect::<Result<Vec<_>, _>>()?,
        total,
    ))
}

pub(crate) async fn find_admin_billing_collector(
    pool: &PostgresPool,
    collector_id: &str,
) -> Result<Option<AdminBillingCollectorRecord>, GatewayError> {
    let row = sqlx::query(
        r#"
SELECT
  id,
  api_format,
  task_type,
  dimension_name,
  source_type,
  source_path,
  value_type,
  transform_expression,
  default_value,
  priority,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM dimension_collectors
WHERE id = $1
        "#,
    )
    .bind(collector_id)
    .fetch_optional(pool)
    .await
    .map_err(internal)?;
    row.as_ref()
        .map(admin_billing_collector_from_row)
        .transpose()
}

pub(crate) async fn update_admin_billing_collector(
    pool: &PostgresPool,
    collector_id: &str,
    input: &AdminBillingCollectorWriteInput,
) -> Result<LocalMutationOutcome<AdminBillingCollectorRecord>, GatewayError> {
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
  id,
  api_format,
  task_type,
  dimension_name,
  source_type,
  source_path,
  value_type,
  transform_expression,
  default_value,
  priority,
  is_enabled,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
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
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(sqlx::Error::Database(err)) => {
            return Ok(LocalMutationOutcome::Invalid(format!(
                "Integrity error: {err}"
            )))
        }
        Err(err) => return Err(GatewayError::Internal(err.to_string())),
    };
    match row {
        Some(row) => Ok(LocalMutationOutcome::Applied(
            admin_billing_collector_from_row(&row)?,
        )),
        None => Ok(LocalMutationOutcome::NotFound),
    }
}

pub(crate) async fn apply_admin_billing_preset(
    pool: &PostgresPool,
    preset: &str,
    mode: &str,
    collectors: &[AdminBillingCollectorWriteInput],
) -> Result<LocalMutationOutcome<AdminBillingPresetApplyResult>, GatewayError> {
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
        .fetch_optional(pool)
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
                .execute(pool)
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
  id,
  api_format,
  task_type,
  dimension_name,
  source_type,
  source_path,
  value_type,
  transform_expression,
  default_value,
  priority,
  is_enabled,
  created_at,
  updated_at
)
VALUES (
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
  NOW(),
  NOW()
)
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
        .execute(pool)
        .await
        {
            Ok(_) => created += 1,
            Err(err) => errors.push(format!(
                "Failed to create collector: api_format={} task_type={} dim={}: {}",
                collector.api_format, collector.task_type, collector.dimension_name, err
            )),
        }
    }

    Ok(LocalMutationOutcome::Applied(
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

fn read_count(row: sqlx::postgres::PgRow) -> Result<u64, GatewayError> {
    Ok(row.try_get::<i64, _>("total").map_err(internal)?.max(0) as u64)
}

fn admin_billing_rule_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminBillingRuleRecord, GatewayError> {
    Ok(AdminBillingRuleRecord {
        id: row.try_get("id").map_err(internal)?,
        name: row.try_get("name").map_err(internal)?,
        task_type: row.try_get("task_type").map_err(internal)?,
        global_model_id: row.try_get("global_model_id").map_err(internal)?,
        model_id: row.try_get("model_id").map_err(internal)?,
        expression: row.try_get("expression").map_err(internal)?,
        variables: row
            .try_get::<Option<serde_json::Value>, _>("variables")
            .map_err(internal)?
            .unwrap_or_else(|| serde_json::json!({})),
        dimension_mappings: row
            .try_get::<Option<serde_json::Value>, _>("dimension_mappings")
            .map_err(internal)?
            .unwrap_or_else(|| serde_json::json!({})),
        is_enabled: row.try_get("is_enabled").map_err(internal)?,
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_err(internal)?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_err(internal)?
            .max(0) as u64,
    })
}

fn admin_billing_collector_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminBillingCollectorRecord, GatewayError> {
    Ok(AdminBillingCollectorRecord {
        id: row.try_get("id").map_err(internal)?,
        api_format: row.try_get("api_format").map_err(internal)?,
        task_type: row.try_get("task_type").map_err(internal)?,
        dimension_name: row.try_get("dimension_name").map_err(internal)?,
        source_type: row.try_get("source_type").map_err(internal)?,
        source_path: row.try_get("source_path").map_err(internal)?,
        value_type: row.try_get("value_type").map_err(internal)?,
        transform_expression: row.try_get("transform_expression").map_err(internal)?,
        default_value: row.try_get("default_value").map_err(internal)?,
        priority: row.try_get("priority").map_err(internal)?,
        is_enabled: row.try_get("is_enabled").map_err(internal)?,
        created_at_unix_secs: row
            .try_get::<i64, _>("created_at_unix_secs")
            .map_err(internal)?
            .max(0) as u64,
        updated_at_unix_secs: row
            .try_get::<i64, _>("updated_at_unix_secs")
            .map_err(internal)?
            .max(0) as u64,
    })
}
