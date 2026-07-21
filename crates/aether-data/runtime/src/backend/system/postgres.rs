use futures_util::TryStreamExt;
use sqlx::Row;

use crate::error::SqlxResultExt;
use crate::repository::system::{
    AdminSystemStats, AdminSystemStatsDailyAggregate, AdminSystemStatsDailyApiKeyAggregate,
    AdminSystemStatsUserDailyAggregate,
};
use crate::DataLayerError;

use super::u64_from_i64;
use super::*;

const FIND_SYSTEM_CONFIG_VALUE_SQL: &str = r#"
SELECT value
FROM system_configs
WHERE key = $1
LIMIT 1
"#;

const UPSERT_SYSTEM_CONFIG_VALUE_SQL: &str = r#"
INSERT INTO system_configs (id, key, value, description, created_at, updated_at)
VALUES ($1, $2, $3, $4, NOW(), NOW())
ON CONFLICT (key) DO UPDATE
SET value = EXCLUDED.value,
    description = COALESCE(EXCLUDED.description, system_configs.description),
    updated_at = NOW()
RETURNING value
"#;

const LIST_SYSTEM_CONFIG_ENTRIES_SQL: &str = r#"
SELECT
    key,
    value,
    description,
    EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM system_configs
ORDER BY key ASC
"#;

const UPSERT_SYSTEM_CONFIG_ENTRY_SQL: &str = r#"
INSERT INTO system_configs (id, key, value, description, created_at, updated_at)
VALUES ($1, $2, $3, $4, NOW(), NOW())
ON CONFLICT (key) DO UPDATE
SET value = EXCLUDED.value,
    description = COALESCE(EXCLUDED.description, system_configs.description),
    updated_at = NOW()
RETURNING
    key,
    value,
    description,
    EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const DELETE_SYSTEM_CONFIG_VALUE_SQL: &str = r#"
DELETE FROM system_configs
WHERE key = $1
"#;

const READ_ADMIN_SYSTEM_STATS_SQL: &str = r#"
SELECT
    (SELECT COUNT(*) FROM users) AS total_users,
    (SELECT COUNT(*) FROM users WHERE is_active IS TRUE) AS active_users,
    (SELECT COUNT(*) FROM api_keys) AS total_api_keys,
    (SELECT COUNT(*) FROM usage) AS total_requests
"#;

async fn export_postgres_admin_system_usage_aggregates(
    pool: &sqlx::PgPool,
) -> Result<AdminSystemUsageAggregateSnapshot, DataLayerError> {
    let mut snapshot = AdminSystemUsageAggregateSnapshot::default();

    let mut daily_rows = sqlx::query(
        r#"
SELECT
    CAST(EXTRACT(EPOCH FROM date) AS BIGINT) AS date_unix_secs,
    COALESCE(total_requests, 0)::BIGINT AS total_requests,
    COALESCE(success_requests, 0)::BIGINT AS success_requests,
    COALESCE(error_requests, 0)::BIGINT AS error_requests,
    COALESCE(input_tokens, 0)::BIGINT AS input_tokens,
    COALESCE(output_tokens, 0)::BIGINT AS output_tokens,
    COALESCE(cache_creation_tokens, 0)::BIGINT AS cache_creation_tokens,
    COALESCE(cache_read_tokens, 0)::BIGINT AS cache_read_tokens,
    COALESCE(CAST(total_cost AS DOUBLE PRECISION), 0) AS total_cost,
    COALESCE(CAST(actual_total_cost AS DOUBLE PRECISION), 0) AS actual_total_cost,
    COALESCE(is_complete, FALSE) AS is_complete,
    CAST(EXTRACT(EPOCH FROM aggregated_at) AS BIGINT) AS aggregated_at_unix_secs
FROM stats_daily
WHERE total_requests <> 0
   OR input_tokens <> 0
   OR output_tokens <> 0
   OR cache_creation_tokens <> 0
   OR cache_read_tokens <> 0
   OR total_cost <> 0
ORDER BY date ASC
"#,
    )
    .fetch(pool);
    while let Some(row) = daily_rows.try_next().await.map_postgres_err()? {
        snapshot.stats_daily.push(map_stats_daily_aggregate(&row)?);
    }

    let mut user_daily_rows = sqlx::query(
        r#"
SELECT
    user_id,
    username,
    CAST(EXTRACT(EPOCH FROM date) AS BIGINT) AS date_unix_secs,
    COALESCE(total_requests, 0)::BIGINT AS total_requests,
    COALESCE(success_requests, 0)::BIGINT AS success_requests,
    COALESCE(error_requests, 0)::BIGINT AS error_requests,
    COALESCE(input_tokens, 0)::BIGINT AS input_tokens,
    COALESCE(output_tokens, 0)::BIGINT AS output_tokens,
    COALESCE(cache_creation_tokens, 0)::BIGINT AS cache_creation_tokens,
    COALESCE(cache_read_tokens, 0)::BIGINT AS cache_read_tokens,
    COALESCE(CAST(total_cost AS DOUBLE PRECISION), 0) AS total_cost
FROM stats_user_daily
WHERE user_id IS NOT NULL
  AND (total_requests <> 0
    OR input_tokens <> 0
    OR output_tokens <> 0
    OR cache_creation_tokens <> 0
    OR cache_read_tokens <> 0
    OR total_cost <> 0)
ORDER BY user_id ASC, date ASC
"#,
    )
    .fetch(pool);
    while let Some(row) = user_daily_rows.try_next().await.map_postgres_err()? {
        snapshot
            .stats_user_daily
            .push(map_stats_user_daily_aggregate(&row)?);
    }

    let mut api_key_daily_rows = sqlx::query(
        r#"
SELECT
    api_key_id,
    api_key_name,
    CAST(EXTRACT(EPOCH FROM date) AS BIGINT) AS date_unix_secs,
    COALESCE(total_requests, 0)::BIGINT AS total_requests,
    COALESCE(success_requests, 0)::BIGINT AS success_requests,
    COALESCE(error_requests, 0)::BIGINT AS error_requests,
    COALESCE(input_tokens, 0)::BIGINT AS input_tokens,
    COALESCE(output_tokens, 0)::BIGINT AS output_tokens,
    COALESCE(cache_creation_tokens, 0)::BIGINT AS cache_creation_tokens,
    COALESCE(cache_read_tokens, 0)::BIGINT AS cache_read_tokens,
    COALESCE(CAST(total_cost AS DOUBLE PRECISION), 0) AS total_cost
FROM stats_daily_api_key
WHERE api_key_id IS NOT NULL
  AND (total_requests <> 0
    OR input_tokens <> 0
    OR output_tokens <> 0
    OR cache_creation_tokens <> 0
    OR cache_read_tokens <> 0
    OR total_cost <> 0)
ORDER BY api_key_id ASC, date ASC
"#,
    )
    .fetch(pool);
    while let Some(row) = api_key_daily_rows.try_next().await.map_postgres_err()? {
        snapshot
            .stats_daily_api_key
            .push(map_stats_daily_api_key_aggregate(&row)?);
    }

    Ok(snapshot)
}

async fn import_postgres_admin_system_usage_aggregates(
    pool: &sqlx::PgPool,
    snapshot: &AdminSystemUsageAggregateSnapshot,
    user_id_map: &BTreeMap<String, String>,
    api_key_id_map: &BTreeMap<String, String>,
    mode: AdminSystemUsageAggregateImportMode,
) -> Result<AdminSystemUsageAggregateImportSummary, DataLayerError> {
    let mut tx = pool.begin().await.map_postgres_err()?;
    let mut summary = AdminSystemUsageAggregateImportSummary::default();

    for row in &snapshot.stats_daily {
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM stats_daily WHERE date = TO_TIMESTAMP($1::double precision) LIMIT 1",
        )
        .bind(i64_from_u64(row.date_unix_secs, "stats_daily.date")?)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if should_skip_imported_aggregate(
            existing.is_some(),
            mode,
            "stats_daily",
            row.date_unix_secs,
        )? {
            summary.stats_daily.skipped += 1;
            continue;
        }
        sqlx::query(
            r#"
INSERT INTO stats_daily (
    id, date, total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, actual_total_cost, is_complete, aggregated_at, created_at, updated_at
)
VALUES (
    $1, TO_TIMESTAMP($2::double precision), $3, $4, $5,
    $6, $7, $8, $9, $10, $11, $12,
    CASE WHEN $13::BIGINT IS NULL THEN NULL ELSE TO_TIMESTAMP($13::double precision) END,
    NOW(), NOW()
)
ON CONFLICT (date) DO UPDATE
SET total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    is_complete = EXCLUDED.is_complete,
    aggregated_at = EXCLUDED.aggregated_at,
    updated_at = NOW()
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(i64_from_u64(row.date_unix_secs, "stats_daily.date")?)
        .bind(i64_from_u64(
            row.total_requests,
            "stats_daily.total_requests",
        )?)
        .bind(i64_from_u64(
            row.success_requests,
            "stats_daily.success_requests",
        )?)
        .bind(i64_from_u64(
            row.error_requests,
            "stats_daily.error_requests",
        )?)
        .bind(i64_from_u64(row.input_tokens, "stats_daily.input_tokens")?)
        .bind(i64_from_u64(
            row.output_tokens,
            "stats_daily.output_tokens",
        )?)
        .bind(i64_from_u64(
            row.cache_creation_tokens,
            "stats_daily.cache_creation_tokens",
        )?)
        .bind(i64_from_u64(
            row.cache_read_tokens,
            "stats_daily.cache_read_tokens",
        )?)
        .bind(row.total_cost)
        .bind(row.actual_total_cost)
        .bind(row.is_complete || row.total_requests > 0)
        .bind(optional_i64_from_u64(
            row.aggregated_at_unix_secs,
            "stats_daily.aggregated_at",
        )?)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        add_aggregate_import_count(&mut summary.stats_daily, existing.is_some());
    }

    for row in &snapshot.stats_user_daily {
        let Some(target_user_id) = user_id_map.get(&row.user_id) else {
            summary.skipped_unmapped_user_daily += 1;
            summary.stats_user_daily.skipped += 1;
            continue;
        };
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM stats_user_daily WHERE user_id = $1 AND date = TO_TIMESTAMP($2::double precision) LIMIT 1",
        )
        .bind(target_user_id)
        .bind(i64_from_u64(row.date_unix_secs, "stats_user_daily.date")?)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if should_skip_imported_aggregate(
            existing.is_some(),
            mode,
            "stats_user_daily",
            row.date_unix_secs,
        )? {
            summary.stats_user_daily.skipped += 1;
            continue;
        }
        sqlx::query(
            r#"
INSERT INTO stats_user_daily (
    id, user_id, username, date, total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
)
VALUES (
    $1, $2, $3, TO_TIMESTAMP($4::double precision), $5, $6, $7,
    $8, $9, $10, $11, $12, NOW(), NOW()
)
ON CONFLICT (date, user_id) DO UPDATE
SET username = EXCLUDED.username,
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = NOW()
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(target_user_id)
        .bind(row.username.as_deref())
        .bind(i64_from_u64(row.date_unix_secs, "stats_user_daily.date")?)
        .bind(i64_from_u64(
            row.total_requests,
            "stats_user_daily.total_requests",
        )?)
        .bind(i64_from_u64(
            row.success_requests,
            "stats_user_daily.success_requests",
        )?)
        .bind(i64_from_u64(
            row.error_requests,
            "stats_user_daily.error_requests",
        )?)
        .bind(i64_from_u64(
            row.input_tokens,
            "stats_user_daily.input_tokens",
        )?)
        .bind(i64_from_u64(
            row.output_tokens,
            "stats_user_daily.output_tokens",
        )?)
        .bind(i64_from_u64(
            row.cache_creation_tokens,
            "stats_user_daily.cache_creation_tokens",
        )?)
        .bind(i64_from_u64(
            row.cache_read_tokens,
            "stats_user_daily.cache_read_tokens",
        )?)
        .bind(row.total_cost)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        add_aggregate_import_count(&mut summary.stats_user_daily, existing.is_some());
    }

    for row in &snapshot.stats_daily_api_key {
        let Some(target_api_key_id) = api_key_id_map.get(&row.api_key_id) else {
            summary.skipped_unmapped_api_key_daily += 1;
            summary.stats_daily_api_key.skipped += 1;
            continue;
        };
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM stats_daily_api_key WHERE api_key_id = $1 AND date = TO_TIMESTAMP($2::double precision) LIMIT 1",
        )
        .bind(target_api_key_id)
        .bind(i64_from_u64(row.date_unix_secs, "stats_daily_api_key.date")?)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if should_skip_imported_aggregate(
            existing.is_some(),
            mode,
            "stats_daily_api_key",
            row.date_unix_secs,
        )? {
            summary.stats_daily_api_key.skipped += 1;
            continue;
        }
        sqlx::query(
            r#"
INSERT INTO stats_daily_api_key (
    id, api_key_id, api_key_name, date, total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
)
VALUES (
    $1, $2, $3, TO_TIMESTAMP($4::double precision), $5, $6, $7,
    $8, $9, $10, $11, $12, NOW(), NOW()
)
ON CONFLICT (date, api_key_id) DO UPDATE
SET api_key_name = EXCLUDED.api_key_name,
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = NOW()
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(target_api_key_id)
        .bind(row.api_key_name.as_deref())
        .bind(i64_from_u64(
            row.date_unix_secs,
            "stats_daily_api_key.date",
        )?)
        .bind(i64_from_u64(
            row.total_requests,
            "stats_daily_api_key.total_requests",
        )?)
        .bind(i64_from_u64(
            row.success_requests,
            "stats_daily_api_key.success_requests",
        )?)
        .bind(i64_from_u64(
            row.error_requests,
            "stats_daily_api_key.error_requests",
        )?)
        .bind(i64_from_u64(
            row.input_tokens,
            "stats_daily_api_key.input_tokens",
        )?)
        .bind(i64_from_u64(
            row.output_tokens,
            "stats_daily_api_key.output_tokens",
        )?)
        .bind(i64_from_u64(
            row.cache_creation_tokens,
            "stats_daily_api_key.cache_creation_tokens",
        )?)
        .bind(i64_from_u64(
            row.cache_read_tokens,
            "stats_daily_api_key.cache_read_tokens",
        )?)
        .bind(row.total_cost)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        add_aggregate_import_count(&mut summary.stats_daily_api_key, existing.is_some());
    }

    tx.commit().await.map_postgres_err()?;
    Ok(summary)
}

async fn purge_postgres_admin_system_data(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target: AdminSystemPurgeTarget,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    match target {
        AdminSystemPurgeTarget::Config => {
            pg_execute_if_table(
                tx,
                "usage",
                "usage_provider_refs_cleared",
                r#"
UPDATE public.usage
SET provider_id = NULL,
    provider_endpoint_id = NULL,
    provider_api_key_id = NULL
WHERE provider_id IS NOT NULL
   OR provider_endpoint_id IS NOT NULL
   OR provider_api_key_id IS NOT NULL
"#,
                summary,
            )
            .await?;
            pg_execute_if_table(
                tx,
                "request_candidates",
                "request_candidate_provider_refs_cleared",
                r#"
UPDATE public.request_candidates
SET provider_id = NULL,
    endpoint_id = NULL,
    key_id = NULL
WHERE provider_id IS NOT NULL
   OR endpoint_id IS NOT NULL
   OR key_id IS NOT NULL
"#,
                summary,
            )
            .await?;
            pg_execute_if_table(
                tx,
                "video_tasks",
                "video_task_provider_refs_cleared",
                r#"
UPDATE public.video_tasks
SET provider_id = NULL,
    endpoint_id = NULL,
    key_id = NULL
WHERE provider_id IS NOT NULL
   OR endpoint_id IS NOT NULL
   OR key_id IS NOT NULL
"#,
                summary,
            )
            .await?;
            pg_execute_if_table(
                tx,
                "user_preferences",
                "user_default_provider_refs_cleared",
                "UPDATE public.user_preferences SET default_provider_id = NULL WHERE default_provider_id IS NOT NULL",
                summary,
            )
            .await?;
            for table in ADMIN_CONFIG_PURGE_TABLES {
                pg_delete_table(tx, table, summary).await?;
            }
        }
        AdminSystemPurgeTarget::Users => {
            purge_postgres_non_admin_users(tx, summary).await?;
        }
        AdminSystemPurgeTarget::Usage => {
            pg_delete_table(tx, "request_candidates", summary).await?;
            for table in ADMIN_USAGE_CHILD_TABLES {
                pg_delete_table(tx, table, summary).await?;
            }
            pg_delete_table(tx, "usage", summary).await?;
            pg_execute_if_table(
                tx,
                "api_keys",
                "api_key_usage_stats_reset",
                r#"
UPDATE public.api_keys
SET total_requests = 0,
    total_tokens = 0,
    total_cost_usd = 0,
    last_used_at = NULL
WHERE total_requests <> 0
   OR total_tokens <> 0
   OR total_cost_usd <> 0
   OR last_used_at IS NOT NULL
"#,
                summary,
            )
            .await?;
            pg_execute_if_table(
                tx,
                "provider_api_keys",
                "provider_key_usage_stats_reset",
                r#"
UPDATE public.provider_api_keys
SET request_count = 0,
    success_count = 0,
    error_count = 0,
    total_tokens = 0,
    total_cost_usd = 0,
    total_response_time_ms = 0,
    last_used_at = NULL
WHERE request_count <> 0
   OR success_count <> 0
   OR error_count <> 0
   OR total_tokens <> 0
   OR total_cost_usd <> 0
   OR total_response_time_ms <> 0
   OR last_used_at IS NOT NULL
"#,
                summary,
            )
            .await?;
        }
        AdminSystemPurgeTarget::AuditLogs => {
            pg_delete_table(tx, "audit_logs", summary).await?;
        }
        AdminSystemPurgeTarget::RequestBodies => {
            pg_delete_table(tx, "usage_body_blobs", summary).await?;
            pg_execute_if_table_has_columns(
                tx,
                "usage",
                USAGE_BODY_FIELD_COLUMNS,
                "usage_body_fields_cleaned",
                r#"
UPDATE public.usage
SET request_body = NULL,
    response_body = NULL,
    provider_request_body = NULL,
    client_response_body = NULL,
    request_body_compressed = NULL,
    response_body_compressed = NULL,
    provider_request_body_compressed = NULL,
    client_response_body_compressed = NULL
WHERE request_body IS NOT NULL
   OR response_body IS NOT NULL
   OR provider_request_body IS NOT NULL
   OR client_response_body IS NOT NULL
   OR request_body_compressed IS NOT NULL
   OR response_body_compressed IS NOT NULL
   OR provider_request_body_compressed IS NOT NULL
   OR client_response_body_compressed IS NOT NULL
"#,
                summary,
            )
            .await?;
            pg_execute_if_table(
                tx,
                "usage_http_audits",
                "usage_http_audit_body_refs_cleaned",
                r#"
UPDATE public.usage_http_audits
SET request_body_ref = NULL,
    provider_request_body_ref = NULL,
    response_body_ref = NULL,
    client_response_body_ref = NULL,
    request_body_state = NULL,
    provider_request_body_state = NULL,
    response_body_state = NULL,
    client_response_body_state = NULL,
    body_capture_mode = 'none',
    updated_at = NOW()
WHERE request_body_ref IS NOT NULL
   OR provider_request_body_ref IS NOT NULL
   OR response_body_ref IS NOT NULL
   OR client_response_body_ref IS NOT NULL
   OR request_body_state IS NOT NULL
   OR provider_request_body_state IS NOT NULL
   OR response_body_state IS NOT NULL
   OR client_response_body_state IS NOT NULL
   OR body_capture_mode <> 'none'
"#,
                summary,
            )
            .await?;
        }
        AdminSystemPurgeTarget::Stats => {
            for table in ADMIN_STATS_PURGE_TABLES {
                pg_delete_table(tx, table, summary).await?;
            }
        }
    }
    Ok(())
}

async fn purge_postgres_non_admin_users(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let non_admin_users = "SELECT id FROM public.users WHERE COALESCE(role::text, '') <> 'admin'";
    let non_admin_keys = "SELECT id FROM public.api_keys WHERE user_id IN (SELECT id FROM public.users WHERE COALESCE(role::text, '') <> 'admin')";

    pg_execute_if_table(
        tx,
        "api_key_provider_mappings",
        "api_key_provider_mappings",
        &format!(
            "DELETE FROM public.api_key_provider_mappings WHERE api_key_id IN ({non_admin_keys})"
        ),
        summary,
    )
    .await?;
    pg_execute_if_table(
        tx,
        "usage",
        "usage_user_refs_cleared",
        &format!(
            r#"
UPDATE public.usage
SET user_id = CASE WHEN user_id IN ({non_admin_users}) THEN NULL ELSE user_id END,
    api_key_id = CASE WHEN api_key_id IN ({non_admin_keys}) THEN NULL ELSE api_key_id END
WHERE user_id IN ({non_admin_users})
   OR api_key_id IN ({non_admin_keys})
"#
        ),
        summary,
    )
    .await?;
    pg_execute_if_table(
        tx,
        "request_candidates",
        "request_candidate_user_refs_cleared",
        &format!(
            r#"
UPDATE public.request_candidates
SET user_id = CASE WHEN user_id IN ({non_admin_users}) THEN NULL ELSE user_id END,
    api_key_id = CASE WHEN api_key_id IN ({non_admin_keys}) THEN NULL ELSE api_key_id END
WHERE user_id IN ({non_admin_users})
   OR api_key_id IN ({non_admin_keys})
"#
        ),
        summary,
    )
    .await?;
    pg_execute_if_table(
        tx,
        "audit_logs",
        "audit_log_user_refs_cleared",
        &format!(
            r#"
UPDATE public.audit_logs
SET user_id = CASE WHEN user_id IN ({non_admin_users}) THEN NULL ELSE user_id END,
    api_key_id = CASE WHEN api_key_id IN ({non_admin_keys}) THEN NULL ELSE api_key_id END
WHERE user_id IN ({non_admin_users})
   OR api_key_id IN ({non_admin_keys})
"#
        ),
        summary,
    )
    .await?;
    pg_execute_if_table(
        tx,
        "video_tasks",
        "video_task_user_refs_cleared",
        &format!(
            r#"
UPDATE public.video_tasks
SET user_id = CASE WHEN user_id IN ({non_admin_users}) THEN NULL ELSE user_id END,
    api_key_id = CASE WHEN api_key_id IN ({non_admin_keys}) THEN NULL ELSE api_key_id END
WHERE user_id IN ({non_admin_users})
   OR api_key_id IN ({non_admin_keys})
"#
        ),
        summary,
    )
    .await?;
    pg_execute_if_table(
        tx,
        "wallets",
        "wallet_user_refs_cleared",
        &format!(
            r#"
UPDATE public.wallets
SET user_id = CASE WHEN user_id IN ({non_admin_users}) THEN NULL ELSE user_id END,
    api_key_id = CASE WHEN api_key_id IN ({non_admin_keys}) THEN NULL ELSE api_key_id END
WHERE user_id IN ({non_admin_users})
   OR api_key_id IN ({non_admin_keys})
"#
        ),
        summary,
    )
    .await?;
    for (table, column, key) in [
        (
            "wallet_transactions",
            "operator_id",
            "wallet_transaction_operator_refs_cleared",
        ),
        (
            "payment_orders",
            "user_id",
            "payment_order_user_refs_cleared",
        ),
        (
            "announcements",
            "author_id",
            "announcement_author_refs_cleared",
        ),
        (
            "proxy_nodes",
            "registered_by",
            "proxy_node_registrant_refs_cleared",
        ),
    ] {
        pg_execute_if_table(
            tx,
            table,
            key,
            &format!(
                "UPDATE public.\"{table}\" SET {column} = NULL WHERE {column} IN ({non_admin_users})"
            ),
            summary,
        )
        .await?;
    }
    pg_execute_if_table(
        tx,
        "refund_requests",
        "refund_request_user_refs_cleared",
        &format!(
            r#"
UPDATE public.refund_requests
SET user_id = CASE WHEN user_id IN ({non_admin_users}) THEN NULL ELSE user_id END,
    requested_by = CASE WHEN requested_by IN ({non_admin_users}) THEN NULL ELSE requested_by END,
    approved_by = CASE WHEN approved_by IN ({non_admin_users}) THEN NULL ELSE approved_by END,
    processed_by = CASE WHEN processed_by IN ({non_admin_users}) THEN NULL ELSE processed_by END
WHERE user_id IN ({non_admin_users})
   OR requested_by IN ({non_admin_users})
   OR approved_by IN ({non_admin_users})
   OR processed_by IN ({non_admin_users})
"#
        ),
        summary,
    )
    .await?;
    pg_delete_non_admin_api_key_rows(tx, "stats_daily_api_key", summary).await?;
    for table in ADMIN_USER_SCOPED_TABLES {
        pg_delete_non_admin_user_rows(tx, table, summary).await?;
    }
    pg_execute_if_table(
        tx,
        "api_keys",
        "api_keys",
        &format!("DELETE FROM public.api_keys WHERE user_id IN ({non_admin_users})"),
        summary,
    )
    .await?;
    pg_execute_if_table(
        tx,
        "users",
        "users",
        "DELETE FROM public.users WHERE COALESCE(role::text, '') <> 'admin'",
        summary,
    )
    .await?;
    Ok(())
}

async fn purge_postgres_request_bodies_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    batch_size: usize,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let limit = i64::try_from(batch_size).unwrap_or(i64::MAX);
    pg_execute_batch_if_table(
        tx,
        "usage_body_blobs",
        "usage_body_blobs",
        r#"
WITH doomed AS (
    SELECT body_ref
    FROM public.usage_body_blobs
    ORDER BY body_ref ASC
    LIMIT $1
)
DELETE FROM public.usage_body_blobs AS blobs
USING doomed
WHERE blobs.body_ref = doomed.body_ref
"#,
        summary,
        limit,
    )
    .await?;
    pg_execute_batch_if_table_has_columns(
        tx,
        "usage",
        USAGE_BODY_FIELD_COLUMNS,
        "usage_body_fields_cleaned",
        r#"
WITH batch AS (
    SELECT request_id
    FROM public.usage
    WHERE request_body IS NOT NULL
       OR response_body IS NOT NULL
       OR provider_request_body IS NOT NULL
       OR client_response_body IS NOT NULL
       OR request_body_compressed IS NOT NULL
       OR response_body_compressed IS NOT NULL
       OR provider_request_body_compressed IS NOT NULL
       OR client_response_body_compressed IS NOT NULL
    ORDER BY created_at_unix_ms ASC, request_id ASC
    LIMIT $1
)
UPDATE public.usage AS usage_rows
SET request_body = NULL,
    response_body = NULL,
    provider_request_body = NULL,
    client_response_body = NULL,
    request_body_compressed = NULL,
    response_body_compressed = NULL,
    provider_request_body_compressed = NULL,
    client_response_body_compressed = NULL
FROM batch
WHERE usage_rows.request_id = batch.request_id
"#,
        summary,
        limit,
    )
    .await?;
    pg_execute_batch_if_table(
        tx,
        "usage_http_audits",
        "usage_http_audit_body_refs_cleaned",
        r#"
WITH batch AS (
    SELECT request_id
    FROM public.usage_http_audits
    WHERE request_body_ref IS NOT NULL
       OR provider_request_body_ref IS NOT NULL
       OR response_body_ref IS NOT NULL
       OR client_response_body_ref IS NOT NULL
       OR request_body_state IS NOT NULL
       OR provider_request_body_state IS NOT NULL
       OR response_body_state IS NOT NULL
       OR client_response_body_state IS NOT NULL
       OR body_capture_mode <> 'none'
    ORDER BY request_id ASC
    LIMIT $1
)
UPDATE public.usage_http_audits AS audits
SET request_body_ref = NULL,
    provider_request_body_ref = NULL,
    response_body_ref = NULL,
    client_response_body_ref = NULL,
    request_body_state = NULL,
    provider_request_body_state = NULL,
    response_body_state = NULL,
    client_response_body_state = NULL,
    body_capture_mode = 'none',
    updated_at = NOW()
FROM batch
WHERE audits.request_id = batch.request_id
"#,
        summary,
        limit,
    )
    .await?;
    Ok(())
}

async fn pg_delete_table(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !pg_table_exists(tx, table).await? {
        return Ok(());
    }
    let sql = format!("DELETE FROM public.\"{table}\"");
    let rows = sqlx::query(&sql)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?
        .rows_affected();
    summary.add(table, rows);
    Ok(())
}

async fn pg_delete_non_admin_user_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !pg_table_exists(tx, table).await? {
        return Ok(());
    }
    let sql = format!(
        "DELETE FROM public.\"{table}\" WHERE user_id IN (SELECT id FROM public.users WHERE COALESCE(role::text, '') <> 'admin')"
    );
    let rows = sqlx::query(&sql)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?
        .rows_affected();
    summary.add(table, rows);
    Ok(())
}

async fn pg_delete_non_admin_api_key_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !pg_table_exists(tx, table).await? {
        return Ok(());
    }
    let sql = format!(
        "DELETE FROM public.\"{table}\" WHERE api_key_id IN (SELECT id FROM public.api_keys WHERE user_id IN (SELECT id FROM public.users WHERE COALESCE(role::text, '') <> 'admin'))"
    );
    let rows = sqlx::query(&sql)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?
        .rows_affected();
    summary.add(table, rows);
    Ok(())
}

async fn pg_execute_if_table(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    if !pg_table_exists(tx, checked_sql_identifier(table)?).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

async fn pg_execute_if_table_has_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    columns: &[&str],
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    if !pg_table_has_columns(tx, checked_sql_identifier(table)?, columns).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

async fn pg_execute_batch_if_table(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
    limit: i64,
) -> Result<(), DataLayerError> {
    if !pg_table_exists(tx, checked_sql_identifier(table)?).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .bind(limit)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

async fn pg_execute_batch_if_table_has_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    columns: &[&str],
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
    limit: i64,
) -> Result<(), DataLayerError> {
    if !pg_table_has_columns(tx, checked_sql_identifier(table)?, columns).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .bind(limit)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

async fn pg_table_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
) -> Result<bool, DataLayerError> {
    let table = checked_sql_identifier(table)?;
    sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("public.{table}"))
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()
}

async fn pg_table_has_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
    columns: &[&str],
) -> Result<bool, DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !pg_table_exists(tx, table).await? {
        return Ok(false);
    }
    for column in columns {
        let column = checked_sql_identifier(column)?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1
                FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = $1
                  AND column_name = $2
            )",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()?;
        if !exists {
            return Ok(false);
        }
    }
    Ok(true)
}

impl PostgresBackend {
    pub async fn purge_admin_system_data(
        &self,
        target: AdminSystemPurgeTarget,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        let mut tx = self.pool().begin().await.map_postgres_err()?;
        let mut summary = AdminSystemPurgeSummary::default();
        purge_postgres_admin_system_data(&mut tx, target, &mut summary).await?;
        tx.commit().await.map_postgres_err()?;
        Ok(summary)
    }

    pub async fn purge_admin_request_bodies_batch(
        &self,
        batch_size: usize,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        if batch_size == 0 {
            return Ok(AdminSystemPurgeSummary::default());
        }
        let mut tx = self.pool().begin().await.map_postgres_err()?;
        let mut summary = AdminSystemPurgeSummary::default();
        purge_postgres_request_bodies_batch(&mut tx, batch_size, &mut summary).await?;
        tx.commit().await.map_postgres_err()?;
        Ok(summary)
    }

    pub async fn find_system_config_value(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let row = sqlx::query(FIND_SYSTEM_CONFIG_VALUE_SQL)
            .bind(key)
            .fetch_optional(self.pool())
            .await
            .map_postgres_err()?;
        row.map(|row| row.try_get("value"))
            .transpose()
            .map_postgres_err()
    }

    pub async fn upsert_system_config_value(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<serde_json::Value, DataLayerError> {
        let row = sqlx::query(UPSERT_SYSTEM_CONFIG_VALUE_SQL)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(key)
            .bind(value)
            .bind(description)
            .fetch_one(self.pool())
            .await
            .map_postgres_err()?;
        row.try_get("value").map_postgres_err()
    }

    pub async fn list_system_config_entries(
        &self,
    ) -> Result<Vec<StoredSystemConfigEntry>, DataLayerError> {
        let mut rows = sqlx::query(LIST_SYSTEM_CONFIG_ENTRIES_SQL).fetch(self.pool());
        let mut entries = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            entries.push(StoredSystemConfigEntry {
                key: row.try_get("key").map_postgres_err()?,
                value: row.try_get("value").map_postgres_err()?,
                description: row.try_get("description").map_postgres_err()?,
                updated_at_unix_secs: row
                    .try_get::<Option<i64>, _>("updated_at_unix_secs")
                    .map_postgres_err()?
                    .map(|value| value.max(0) as u64),
            });
        }
        Ok(entries)
    }

    pub async fn upsert_system_config_entry(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<StoredSystemConfigEntry, DataLayerError> {
        let row = sqlx::query(UPSERT_SYSTEM_CONFIG_ENTRY_SQL)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(key)
            .bind(value)
            .bind(description)
            .fetch_one(self.pool())
            .await
            .map_postgres_err()?;
        Ok(StoredSystemConfigEntry {
            key: row.try_get("key").map_postgres_err()?,
            value: row.try_get("value").map_postgres_err()?,
            description: row.try_get("description").map_postgres_err()?,
            updated_at_unix_secs: row
                .try_get::<Option<i64>, _>("updated_at_unix_secs")
                .map_postgres_err()?
                .map(|value| value.max(0) as u64),
        })
    }

    pub async fn delete_system_config_value(&self, key: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query(DELETE_SYSTEM_CONFIG_VALUE_SQL)
            .bind(key)
            .execute(self.pool())
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn read_admin_system_stats(&self) -> Result<AdminSystemStats, DataLayerError> {
        let row = sqlx::query(READ_ADMIN_SYSTEM_STATS_SQL)
            .fetch_one(self.pool())
            .await
            .map_postgres_err()?;
        map_admin_system_stats(row)
    }

    pub async fn export_admin_system_usage_aggregates(
        &self,
    ) -> Result<AdminSystemUsageAggregateSnapshot, DataLayerError> {
        export_postgres_admin_system_usage_aggregates(self.pool()).await
    }

    pub async fn import_admin_system_usage_aggregates(
        &self,
        snapshot: &AdminSystemUsageAggregateSnapshot,
        user_id_map: &BTreeMap<String, String>,
        api_key_id_map: &BTreeMap<String, String>,
        mode: AdminSystemUsageAggregateImportMode,
    ) -> Result<AdminSystemUsageAggregateImportSummary, DataLayerError> {
        import_postgres_admin_system_usage_aggregates(
            self.pool(),
            snapshot,
            user_id_map,
            api_key_id_map,
            mode,
        )
        .await
    }
}

pub(super) fn map_stats_daily_aggregate(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminSystemStatsDailyAggregate, DataLayerError> {
    Ok(AdminSystemStatsDailyAggregate {
        date_unix_secs: u64_from_i64(row.try_get("date_unix_secs").map_postgres_err()?),
        total_requests: u64_from_i64(row.try_get("total_requests").map_postgres_err()?),
        success_requests: u64_from_i64(row.try_get("success_requests").map_postgres_err()?),
        error_requests: u64_from_i64(row.try_get("error_requests").map_postgres_err()?),
        input_tokens: u64_from_i64(row.try_get("input_tokens").map_postgres_err()?),
        output_tokens: u64_from_i64(row.try_get("output_tokens").map_postgres_err()?),
        cache_creation_tokens: u64_from_i64(
            row.try_get("cache_creation_tokens").map_postgres_err()?,
        ),
        cache_read_tokens: u64_from_i64(row.try_get("cache_read_tokens").map_postgres_err()?),
        total_cost: row.try_get("total_cost").map_postgres_err()?,
        actual_total_cost: row.try_get("actual_total_cost").map_postgres_err()?,
        is_complete: row.try_get("is_complete").map_postgres_err()?,
        aggregated_at_unix_secs: row
            .try_get::<Option<i64>, _>("aggregated_at_unix_secs")
            .map_postgres_err()?
            .map(u64_from_i64),
    })
}

pub(super) fn map_stats_user_daily_aggregate(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminSystemStatsUserDailyAggregate, DataLayerError> {
    Ok(AdminSystemStatsUserDailyAggregate {
        user_id: row.try_get("user_id").map_postgres_err()?,
        username: row.try_get("username").map_postgres_err()?,
        date_unix_secs: u64_from_i64(row.try_get("date_unix_secs").map_postgres_err()?),
        total_requests: u64_from_i64(row.try_get("total_requests").map_postgres_err()?),
        success_requests: u64_from_i64(row.try_get("success_requests").map_postgres_err()?),
        error_requests: u64_from_i64(row.try_get("error_requests").map_postgres_err()?),
        input_tokens: u64_from_i64(row.try_get("input_tokens").map_postgres_err()?),
        output_tokens: u64_from_i64(row.try_get("output_tokens").map_postgres_err()?),
        cache_creation_tokens: u64_from_i64(
            row.try_get("cache_creation_tokens").map_postgres_err()?,
        ),
        cache_read_tokens: u64_from_i64(row.try_get("cache_read_tokens").map_postgres_err()?),
        total_cost: row.try_get("total_cost").map_postgres_err()?,
    })
}

pub(super) fn map_stats_daily_api_key_aggregate(
    row: &sqlx::postgres::PgRow,
) -> Result<AdminSystemStatsDailyApiKeyAggregate, DataLayerError> {
    Ok(AdminSystemStatsDailyApiKeyAggregate {
        api_key_id: row.try_get("api_key_id").map_postgres_err()?,
        api_key_name: row.try_get("api_key_name").map_postgres_err()?,
        date_unix_secs: u64_from_i64(row.try_get("date_unix_secs").map_postgres_err()?),
        total_requests: u64_from_i64(row.try_get("total_requests").map_postgres_err()?),
        success_requests: u64_from_i64(row.try_get("success_requests").map_postgres_err()?),
        error_requests: u64_from_i64(row.try_get("error_requests").map_postgres_err()?),
        input_tokens: u64_from_i64(row.try_get("input_tokens").map_postgres_err()?),
        output_tokens: u64_from_i64(row.try_get("output_tokens").map_postgres_err()?),
        cache_creation_tokens: u64_from_i64(
            row.try_get("cache_creation_tokens").map_postgres_err()?,
        ),
        cache_read_tokens: u64_from_i64(row.try_get("cache_read_tokens").map_postgres_err()?),
        total_cost: row.try_get("total_cost").map_postgres_err()?,
    })
}

pub(super) fn map_admin_system_stats(
    row: sqlx::postgres::PgRow,
) -> Result<AdminSystemStats, DataLayerError> {
    Ok(AdminSystemStats {
        total_users: row
            .try_get::<i64, _>("total_users")
            .map_postgres_err()?
            .max(0) as u64,
        active_users: row
            .try_get::<i64, _>("active_users")
            .map_postgres_err()?
            .max(0) as u64,
        total_api_keys: row
            .try_get::<i64, _>("total_api_keys")
            .map_postgres_err()?
            .max(0) as u64,
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_postgres_err()?
            .max(0) as u64,
    })
}
