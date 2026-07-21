use sqlx::Row;

use crate::error::SqlResultExt;
use crate::repository::system::{
    AdminSystemStats, AdminSystemStatsDailyAggregate, AdminSystemStatsDailyApiKeyAggregate,
    AdminSystemStatsUserDailyAggregate,
};
use crate::DataLayerError;

use super::u64_from_i64;
use super::*;

const READ_ADMIN_SYSTEM_STATS_SQL: &str = r#"
SELECT
    (SELECT COUNT(*) FROM users) AS total_users,
    (SELECT COUNT(*) FROM users WHERE is_active = 1) AS active_users,
    (SELECT COUNT(*) FROM api_keys) AS total_api_keys,
    (SELECT COUNT(*) FROM "usage") AS total_requests
"#;

async fn export_sqlite_admin_system_usage_aggregates(
    pool: &sqlx::SqlitePool,
) -> Result<AdminSystemUsageAggregateSnapshot, DataLayerError> {
    let mut snapshot = AdminSystemUsageAggregateSnapshot::default();

    let daily_rows = sqlx::query(
        r#"
SELECT
    "date" AS date_unix_secs,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    is_complete,
    aggregated_at AS aggregated_at_unix_secs
FROM stats_daily
WHERE total_requests <> 0
   OR input_tokens <> 0
   OR output_tokens <> 0
   OR cache_creation_tokens <> 0
   OR cache_read_tokens <> 0
   OR total_cost <> 0
ORDER BY "date" ASC
"#,
    )
    .fetch_all(pool)
    .await
    .map_sql_err()?;
    for row in daily_rows {
        snapshot.stats_daily.push(map_stats_daily_aggregate(&row)?);
    }

    let user_daily_rows = sqlx::query(
        r#"
SELECT
    user_id,
    username,
    "date" AS date_unix_secs,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost
FROM stats_user_daily
WHERE total_requests <> 0
   OR input_tokens <> 0
   OR output_tokens <> 0
   OR cache_creation_tokens <> 0
   OR cache_read_tokens <> 0
   OR total_cost <> 0
ORDER BY user_id ASC, "date" ASC
"#,
    )
    .fetch_all(pool)
    .await
    .map_sql_err()?;
    for row in user_daily_rows {
        snapshot
            .stats_user_daily
            .push(map_stats_user_daily_aggregate(&row)?);
    }

    let api_key_daily_rows = sqlx::query(
        r#"
SELECT
    api_key_id,
    api_key_name,
    "date" AS date_unix_secs,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost
FROM stats_daily_api_key
WHERE total_requests <> 0
   OR input_tokens <> 0
   OR output_tokens <> 0
   OR cache_creation_tokens <> 0
   OR cache_read_tokens <> 0
   OR total_cost <> 0
ORDER BY api_key_id ASC, "date" ASC
"#,
    )
    .fetch_all(pool)
    .await
    .map_sql_err()?;
    for row in api_key_daily_rows {
        snapshot
            .stats_daily_api_key
            .push(map_stats_daily_api_key_aggregate(&row)?);
    }

    Ok(snapshot)
}

async fn import_sqlite_admin_system_usage_aggregates(
    pool: &sqlx::SqlitePool,
    snapshot: &AdminSystemUsageAggregateSnapshot,
    user_id_map: &BTreeMap<String, String>,
    api_key_id_map: &BTreeMap<String, String>,
    mode: AdminSystemUsageAggregateImportMode,
) -> Result<AdminSystemUsageAggregateImportSummary, DataLayerError> {
    let mut tx = pool.begin().await.map_sql_err()?;
    let mut summary = AdminSystemUsageAggregateImportSummary::default();
    let now = current_unix_secs();

    for row in &snapshot.stats_daily {
        let existing: Option<String> =
            sqlx::query_scalar(r#"SELECT id FROM stats_daily WHERE "date" = ? LIMIT 1"#)
                .bind(i64_from_u64(row.date_unix_secs, "stats_daily.date")?)
                .fetch_optional(&mut *tx)
                .await
                .map_sql_err()?;
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
    id, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, actual_total_cost, is_complete, aggregated_at, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT ("date") DO UPDATE
SET total_requests = excluded.total_requests,
    success_requests = excluded.success_requests,
    error_requests = excluded.error_requests,
    input_tokens = excluded.input_tokens,
    output_tokens = excluded.output_tokens,
    cache_creation_tokens = excluded.cache_creation_tokens,
    cache_read_tokens = excluded.cache_read_tokens,
    total_cost = excluded.total_cost,
    actual_total_cost = excluded.actual_total_cost,
    is_complete = excluded.is_complete,
    aggregated_at = excluded.aggregated_at,
    updated_at = excluded.updated_at
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
        .bind(if row.is_complete || row.total_requests > 0 {
            1_i64
        } else {
            0_i64
        })
        .bind(optional_i64_from_u64(
            row.aggregated_at_unix_secs,
            "stats_daily.aggregated_at",
        )?)
        .bind(i64_from_u64(now, "stats_daily.created_at")?)
        .bind(i64_from_u64(now, "stats_daily.updated_at")?)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        add_aggregate_import_count(&mut summary.stats_daily, existing.is_some());
    }

    for row in &snapshot.stats_user_daily {
        let Some(target_user_id) = user_id_map.get(&row.user_id) else {
            summary.skipped_unmapped_user_daily += 1;
            summary.stats_user_daily.skipped += 1;
            continue;
        };
        let existing: Option<String> = sqlx::query_scalar(
            r#"SELECT id FROM stats_user_daily WHERE user_id = ? AND "date" = ? LIMIT 1"#,
        )
        .bind(target_user_id)
        .bind(i64_from_u64(row.date_unix_secs, "stats_user_daily.date")?)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
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
    id, user_id, username, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT ("date", user_id) DO UPDATE
SET username = excluded.username,
    total_requests = excluded.total_requests,
    success_requests = excluded.success_requests,
    error_requests = excluded.error_requests,
    input_tokens = excluded.input_tokens,
    output_tokens = excluded.output_tokens,
    cache_creation_tokens = excluded.cache_creation_tokens,
    cache_read_tokens = excluded.cache_read_tokens,
    total_cost = excluded.total_cost,
    updated_at = excluded.updated_at
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
        .bind(i64_from_u64(now, "stats_user_daily.created_at")?)
        .bind(i64_from_u64(now, "stats_user_daily.updated_at")?)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        add_aggregate_import_count(&mut summary.stats_user_daily, existing.is_some());
    }

    for row in &snapshot.stats_daily_api_key {
        let Some(target_api_key_id) = api_key_id_map.get(&row.api_key_id) else {
            summary.skipped_unmapped_api_key_daily += 1;
            summary.stats_daily_api_key.skipped += 1;
            continue;
        };
        let existing: Option<String> = sqlx::query_scalar(
            r#"SELECT id FROM stats_daily_api_key WHERE api_key_id = ? AND "date" = ? LIMIT 1"#,
        )
        .bind(target_api_key_id)
        .bind(i64_from_u64(
            row.date_unix_secs,
            "stats_daily_api_key.date",
        )?)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
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
    id, api_key_id, api_key_name, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT ("date", api_key_id) DO UPDATE
SET api_key_name = excluded.api_key_name,
    total_requests = excluded.total_requests,
    success_requests = excluded.success_requests,
    error_requests = excluded.error_requests,
    input_tokens = excluded.input_tokens,
    output_tokens = excluded.output_tokens,
    cache_creation_tokens = excluded.cache_creation_tokens,
    cache_read_tokens = excluded.cache_read_tokens,
    total_cost = excluded.total_cost,
    updated_at = excluded.updated_at
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
        .bind(i64_from_u64(now, "stats_daily_api_key.created_at")?)
        .bind(i64_from_u64(now, "stats_daily_api_key.updated_at")?)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        add_aggregate_import_count(&mut summary.stats_daily_api_key, existing.is_some());
    }

    tx.commit().await.map_sql_err()?;
    Ok(summary)
}

pub(super) async fn sqlite_delete_table(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !sqlite_table_exists(tx, table).await? {
        return Ok(());
    }
    let sql = format!("DELETE FROM \"{table}\"");
    let rows = sqlx::query(&sql)
        .execute(&mut **tx)
        .await
        .map_sql_err()?
        .rows_affected();
    summary.add(table, rows);
    Ok(())
}

pub(super) async fn sqlite_delete_non_admin_user_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !sqlite_table_exists(tx, table).await? {
        return Ok(());
    }
    let sql = format!(
        "DELETE FROM \"{table}\" WHERE user_id IN (SELECT id FROM users WHERE LOWER(COALESCE(role, '')) <> 'admin')"
    );
    let rows = sqlx::query(&sql)
        .execute(&mut **tx)
        .await
        .map_sql_err()?
        .rows_affected();
    summary.add(table, rows);
    Ok(())
}

pub(super) async fn sqlite_delete_non_admin_api_key_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !sqlite_table_exists(tx, table).await? {
        return Ok(());
    }
    let sql = format!(
        "DELETE FROM \"{table}\" WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id IN (SELECT id FROM users WHERE LOWER(COALESCE(role, '')) <> 'admin'))"
    );
    let rows = sqlx::query(&sql)
        .execute(&mut **tx)
        .await
        .map_sql_err()?
        .rows_affected();
    summary.add(table, rows);
    Ok(())
}

pub(super) async fn sqlite_execute_if_table(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    if !sqlite_table_exists(tx, checked_sql_identifier(table)?).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .execute(&mut **tx)
        .await
        .map_sql_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

pub(super) async fn sqlite_execute_if_table_has_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    columns: &[&str],
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    if !sqlite_table_has_columns(tx, checked_sql_identifier(table)?, columns).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .execute(&mut **tx)
        .await
        .map_sql_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

pub(super) async fn sqlite_execute_batch_if_table(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
    limit: i64,
) -> Result<(), DataLayerError> {
    if !sqlite_table_exists(tx, checked_sql_identifier(table)?).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .bind(limit)
        .execute(&mut **tx)
        .await
        .map_sql_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

pub(super) async fn sqlite_execute_batch_if_table_has_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    columns: &[&str],
    key: &str,
    sql: &str,
    summary: &mut AdminSystemPurgeSummary,
    limit: i64,
) -> Result<(), DataLayerError> {
    if !sqlite_table_has_columns(tx, checked_sql_identifier(table)?, columns).await? {
        return Ok(());
    }
    let rows = sqlx::query(sql)
        .bind(limit)
        .execute(&mut **tx)
        .await
        .map_sql_err()?
        .rows_affected();
    summary.add(key, rows);
    Ok(())
}

pub(super) async fn sqlite_table_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
) -> Result<bool, DataLayerError> {
    let table = checked_sql_identifier(table)?;
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_one(&mut **tx)
            .await
            .map_sql_err()?;
    Ok(total > 0)
}

pub(super) async fn sqlite_table_has_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    columns: &[&str],
) -> Result<bool, DataLayerError> {
    let table = checked_sql_identifier(table)?;
    if !sqlite_table_exists(tx, table).await? {
        return Ok(false);
    }
    for column in columns {
        let column = checked_sql_identifier(column)?;
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?")
                .bind(table)
                .bind(column)
                .fetch_one(&mut **tx)
                .await
                .map_sql_err()?;
        if total == 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn purge_sqlite_admin_system_data(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target: AdminSystemPurgeTarget,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    match target {
        AdminSystemPurgeTarget::Config => {
            sqlite_execute_if_table(
                tx,
                "usage",
                "usage_provider_refs_cleared",
                r#"
UPDATE "usage"
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
            sqlite_execute_if_table(
                tx,
                "request_candidates",
                "request_candidate_provider_refs_cleared",
                r#"
UPDATE request_candidates
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
            sqlite_execute_if_table(
                tx,
                "video_tasks",
                "video_task_provider_refs_cleared",
                r#"
UPDATE video_tasks
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
            sqlite_execute_if_table(
                tx,
                "user_preferences",
                "user_default_provider_refs_cleared",
                "UPDATE user_preferences SET default_provider_id = NULL WHERE default_provider_id IS NOT NULL",
                summary,
            )
            .await?;
            for table in ADMIN_CONFIG_PURGE_TABLES {
                sqlite_delete_table(tx, table, summary).await?;
            }
        }
        AdminSystemPurgeTarget::Users => purge_sqlite_non_admin_users(tx, summary).await?,
        AdminSystemPurgeTarget::Usage => {
            sqlite_delete_table(tx, "request_candidates", summary).await?;
            for table in ADMIN_USAGE_CHILD_TABLES {
                sqlite_delete_table(tx, table, summary).await?;
            }
            sqlite_delete_table(tx, "usage", summary).await?;
            sqlite_execute_if_table(
                tx,
                "api_keys",
                "api_key_usage_stats_reset",
                r#"
UPDATE api_keys
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
            sqlite_execute_if_table(
                tx,
                "provider_api_keys",
                "provider_key_usage_stats_reset",
                r#"
UPDATE provider_api_keys
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
            sqlite_delete_table(tx, "audit_logs", summary).await?;
        }
        AdminSystemPurgeTarget::RequestBodies => {
            sqlite_delete_table(tx, "usage_body_blobs", summary).await?;
            sqlite_execute_if_table_has_columns(
                tx,
                "usage",
                USAGE_BODY_FIELD_COLUMNS,
                "usage_body_fields_cleaned",
                r#"
UPDATE "usage"
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
            sqlite_execute_if_table(
                tx,
                "usage_http_audits",
                "usage_http_audit_body_refs_cleaned",
                r#"
UPDATE usage_http_audits
SET request_body_ref = NULL,
    provider_request_body_ref = NULL,
    response_body_ref = NULL,
    client_response_body_ref = NULL,
    request_body_state = NULL,
    provider_request_body_state = NULL,
    response_body_state = NULL,
    client_response_body_state = NULL,
    body_capture_mode = 'none'
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
                sqlite_delete_table(tx, table, summary).await?;
            }
        }
    }
    Ok(())
}

async fn purge_sqlite_non_admin_users(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let non_admin_users = "SELECT id FROM users WHERE LOWER(COALESCE(role, '')) <> 'admin'";
    let non_admin_keys = "SELECT id FROM api_keys WHERE user_id IN (SELECT id FROM users WHERE LOWER(COALESCE(role, '')) <> 'admin')";

    sqlite_execute_if_table(
        tx,
        "api_key_provider_mappings",
        "api_key_provider_mappings",
        &format!("DELETE FROM api_key_provider_mappings WHERE api_key_id IN ({non_admin_keys})"),
        summary,
    )
    .await?;
    sqlite_execute_if_table(
        tx,
        "usage",
        "usage_user_refs_cleared",
        &format!(r#"UPDATE "usage" SET user_id = NULL WHERE user_id IN ({non_admin_users})"#),
        summary,
    )
    .await?;
    sqlite_execute_if_table(
        tx,
        "usage",
        "usage_api_key_refs_cleared",
        &format!(r#"UPDATE "usage" SET api_key_id = NULL WHERE api_key_id IN ({non_admin_keys})"#),
        summary,
    )
    .await?;
    for (table, column, key) in [
        (
            "request_candidates",
            "user_id",
            "request_candidate_user_refs_cleared",
        ),
        ("audit_logs", "user_id", "audit_log_user_refs_cleared"),
        ("video_tasks", "user_id", "video_task_user_refs_cleared"),
        ("wallets", "user_id", "wallet_user_refs_cleared"),
        (
            "payment_orders",
            "user_id",
            "payment_order_user_refs_cleared",
        ),
        (
            "wallet_transactions",
            "operator_id",
            "wallet_transaction_operator_refs_cleared",
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
        sqlite_execute_if_table(
            tx,
            table,
            key,
            &format!(
                r#"UPDATE "{table}" SET "{column}" = NULL WHERE "{column}" IN ({non_admin_users})"#
            ),
            summary,
        )
        .await?;
    }
    for (table, key) in [
        (
            "request_candidates",
            "request_candidate_api_key_refs_cleared",
        ),
        ("audit_logs", "audit_log_api_key_refs_cleared"),
        ("video_tasks", "video_task_api_key_refs_cleared"),
        ("wallets", "wallet_api_key_refs_cleared"),
    ] {
        sqlite_execute_if_table(
            tx,
            table,
            key,
            &format!(
                r#"UPDATE "{table}" SET api_key_id = NULL WHERE api_key_id IN ({non_admin_keys})"#
            ),
            summary,
        )
        .await?;
    }
    sqlite_delete_non_admin_api_key_rows(tx, "stats_daily_api_key", summary).await?;
    sqlite_execute_if_table(
        tx,
        "refund_requests",
        "refund_request_user_refs_cleared",
        &format!(
            r#"
UPDATE refund_requests
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
    for table in ADMIN_USER_SCOPED_TABLES {
        sqlite_delete_non_admin_user_rows(tx, table, summary).await?;
    }
    sqlite_execute_if_table(
        tx,
        "api_keys",
        "api_keys",
        &format!("DELETE FROM api_keys WHERE user_id IN ({non_admin_users})"),
        summary,
    )
    .await?;
    sqlite_execute_if_table(
        tx,
        "users",
        "users",
        "DELETE FROM users WHERE LOWER(COALESCE(role, '')) <> 'admin'",
        summary,
    )
    .await?;
    Ok(())
}

async fn purge_sqlite_request_bodies_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    batch_size: usize,
    summary: &mut AdminSystemPurgeSummary,
) -> Result<(), DataLayerError> {
    let limit = i64::try_from(batch_size).unwrap_or(i64::MAX);
    sqlite_execute_batch_if_table(
        tx,
        "usage_body_blobs",
        "usage_body_blobs",
        r#"
DELETE FROM usage_body_blobs
WHERE body_ref IN (
    SELECT body_ref
    FROM usage_body_blobs
    ORDER BY body_ref ASC
    LIMIT ?
)
"#,
        summary,
        limit,
    )
    .await?;
    sqlite_execute_batch_if_table_has_columns(
        tx,
        "usage",
        USAGE_BODY_FIELD_COLUMNS,
        "usage_body_fields_cleaned",
        r#"
UPDATE "usage"
SET request_body = NULL,
    response_body = NULL,
    provider_request_body = NULL,
    client_response_body = NULL,
    request_body_compressed = NULL,
    response_body_compressed = NULL,
    provider_request_body_compressed = NULL,
    client_response_body_compressed = NULL
WHERE request_id IN (
    SELECT request_id
    FROM "usage"
    WHERE request_body IS NOT NULL
       OR response_body IS NOT NULL
       OR provider_request_body IS NOT NULL
       OR client_response_body IS NOT NULL
       OR request_body_compressed IS NOT NULL
       OR response_body_compressed IS NOT NULL
       OR provider_request_body_compressed IS NOT NULL
       OR client_response_body_compressed IS NOT NULL
    ORDER BY created_at_unix_ms ASC, request_id ASC
    LIMIT ?
)
"#,
        summary,
        limit,
    )
    .await?;
    sqlite_execute_batch_if_table(
        tx,
        "usage_http_audits",
        "usage_http_audit_body_refs_cleaned",
        r#"
UPDATE usage_http_audits
SET request_body_ref = NULL,
    provider_request_body_ref = NULL,
    response_body_ref = NULL,
    client_response_body_ref = NULL,
    request_body_state = NULL,
    provider_request_body_state = NULL,
    response_body_state = NULL,
    client_response_body_state = NULL,
    body_capture_mode = 'none'
WHERE request_id IN (
    SELECT request_id
    FROM usage_http_audits
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
    LIMIT ?
)
"#,
        summary,
        limit,
    )
    .await?;
    Ok(())
}

impl SqliteBackend {
    pub async fn purge_admin_system_data(
        &self,
        target: AdminSystemPurgeTarget,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        let mut tx = self.pool().begin().await.map_sql_err()?;
        let mut summary = AdminSystemPurgeSummary::default();
        purge_sqlite_admin_system_data(&mut tx, target, &mut summary).await?;
        tx.commit().await.map_sql_err()?;
        Ok(summary)
    }

    pub async fn purge_admin_request_bodies_batch(
        &self,
        batch_size: usize,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        if batch_size == 0 {
            return Ok(AdminSystemPurgeSummary::default());
        }
        let mut tx = self.pool().begin().await.map_sql_err()?;
        let mut summary = AdminSystemPurgeSummary::default();
        purge_sqlite_request_bodies_batch(&mut tx, batch_size, &mut summary).await?;
        tx.commit().await.map_sql_err()?;
        Ok(summary)
    }

    pub async fn find_system_config_value(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT value
FROM system_configs
WHERE key = ?
LIMIT 1
"#,
        )
        .bind(key)
        .fetch_optional(self.pool())
        .await
        .map_sql_err()?;

        row.map(|row| {
            row.try_get("value")
                .map_sql_err()
                .and_then(parse_json_value)
        })
        .transpose()
    }

    pub async fn upsert_system_config_value(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<serde_json::Value, DataLayerError> {
        Ok(self
            .upsert_system_config_entry(key, value, description)
            .await?
            .value)
    }

    pub async fn list_system_config_entries(
        &self,
    ) -> Result<Vec<StoredSystemConfigEntry>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT key, value, description, updated_at
FROM system_configs
ORDER BY key ASC
"#,
        )
        .fetch_all(self.pool())
        .await
        .map_sql_err()?;

        rows.into_iter()
            .map(|row| {
                Ok(StoredSystemConfigEntry {
                    key: row.try_get("key").map_sql_err()?,
                    value: parse_json_value(row.try_get("value").map_sql_err()?)?,
                    description: row.try_get("description").map_sql_err()?,
                    updated_at_unix_secs: row
                        .try_get::<Option<i64>, _>("updated_at")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                })
            })
            .collect()
    }

    pub async fn upsert_system_config_entry(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<StoredSystemConfigEntry, DataLayerError> {
        let now = current_unix_secs();
        let serialized = serialize_json_value(value)?;
        sqlx::query(
            r#"
INSERT INTO system_configs (id, key, value, description, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?)
ON CONFLICT (key) DO UPDATE
SET value = excluded.value,
    description = COALESCE(excluded.description, system_configs.description),
    updated_at = excluded.updated_at
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(key)
        .bind(serialized)
        .bind(description)
        .bind(now as i64)
        .bind(now as i64)
        .execute(self.pool())
        .await
        .map_sql_err()?;

        self.list_system_config_entries()
            .await?
            .into_iter()
            .find(|entry| entry.key == key)
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "system config key '{key}' missing after sqlite upsert"
                ))
            })
    }

    pub async fn delete_system_config_value(&self, key: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
DELETE FROM system_configs
WHERE key = ?
"#,
        )
        .bind(key)
        .execute(self.pool())
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn read_admin_system_stats(&self) -> Result<AdminSystemStats, DataLayerError> {
        let row = sqlx::query(READ_ADMIN_SYSTEM_STATS_SQL)
            .fetch_one(self.pool())
            .await
            .map_sql_err()?;
        map_admin_system_stats(row)
    }

    pub async fn export_admin_system_usage_aggregates(
        &self,
    ) -> Result<AdminSystemUsageAggregateSnapshot, DataLayerError> {
        export_sqlite_admin_system_usage_aggregates(self.pool()).await
    }

    pub async fn import_admin_system_usage_aggregates(
        &self,
        snapshot: &AdminSystemUsageAggregateSnapshot,
        user_id_map: &BTreeMap<String, String>,
        api_key_id_map: &BTreeMap<String, String>,
        mode: AdminSystemUsageAggregateImportMode,
    ) -> Result<AdminSystemUsageAggregateImportSummary, DataLayerError> {
        import_sqlite_admin_system_usage_aggregates(
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
    row: &sqlx::sqlite::SqliteRow,
) -> Result<AdminSystemStatsDailyAggregate, DataLayerError> {
    Ok(AdminSystemStatsDailyAggregate {
        date_unix_secs: u64_from_i64(row.try_get("date_unix_secs").map_sql_err()?),
        total_requests: u64_from_i64(row.try_get("total_requests").map_sql_err()?),
        success_requests: u64_from_i64(row.try_get("success_requests").map_sql_err()?),
        error_requests: u64_from_i64(row.try_get("error_requests").map_sql_err()?),
        input_tokens: u64_from_i64(row.try_get("input_tokens").map_sql_err()?),
        output_tokens: u64_from_i64(row.try_get("output_tokens").map_sql_err()?),
        cache_creation_tokens: u64_from_i64(row.try_get("cache_creation_tokens").map_sql_err()?),
        cache_read_tokens: u64_from_i64(row.try_get("cache_read_tokens").map_sql_err()?),
        total_cost: row.try_get("total_cost").map_sql_err()?,
        actual_total_cost: row.try_get("actual_total_cost").map_sql_err()?,
        is_complete: row.try_get::<i64, _>("is_complete").map_sql_err()? != 0,
        aggregated_at_unix_secs: row
            .try_get::<Option<i64>, _>("aggregated_at_unix_secs")
            .map_sql_err()?
            .map(u64_from_i64),
    })
}

pub(super) fn map_stats_user_daily_aggregate(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<AdminSystemStatsUserDailyAggregate, DataLayerError> {
    Ok(AdminSystemStatsUserDailyAggregate {
        user_id: row.try_get("user_id").map_sql_err()?,
        username: row.try_get("username").map_sql_err()?,
        date_unix_secs: u64_from_i64(row.try_get("date_unix_secs").map_sql_err()?),
        total_requests: u64_from_i64(row.try_get("total_requests").map_sql_err()?),
        success_requests: u64_from_i64(row.try_get("success_requests").map_sql_err()?),
        error_requests: u64_from_i64(row.try_get("error_requests").map_sql_err()?),
        input_tokens: u64_from_i64(row.try_get("input_tokens").map_sql_err()?),
        output_tokens: u64_from_i64(row.try_get("output_tokens").map_sql_err()?),
        cache_creation_tokens: u64_from_i64(row.try_get("cache_creation_tokens").map_sql_err()?),
        cache_read_tokens: u64_from_i64(row.try_get("cache_read_tokens").map_sql_err()?),
        total_cost: row.try_get("total_cost").map_sql_err()?,
    })
}

pub(super) fn map_stats_daily_api_key_aggregate(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<AdminSystemStatsDailyApiKeyAggregate, DataLayerError> {
    Ok(AdminSystemStatsDailyApiKeyAggregate {
        api_key_id: row.try_get("api_key_id").map_sql_err()?,
        api_key_name: row.try_get("api_key_name").map_sql_err()?,
        date_unix_secs: u64_from_i64(row.try_get("date_unix_secs").map_sql_err()?),
        total_requests: u64_from_i64(row.try_get("total_requests").map_sql_err()?),
        success_requests: u64_from_i64(row.try_get("success_requests").map_sql_err()?),
        error_requests: u64_from_i64(row.try_get("error_requests").map_sql_err()?),
        input_tokens: u64_from_i64(row.try_get("input_tokens").map_sql_err()?),
        output_tokens: u64_from_i64(row.try_get("output_tokens").map_sql_err()?),
        cache_creation_tokens: u64_from_i64(row.try_get("cache_creation_tokens").map_sql_err()?),
        cache_read_tokens: u64_from_i64(row.try_get("cache_read_tokens").map_sql_err()?),
        total_cost: row.try_get("total_cost").map_sql_err()?,
    })
}

pub(super) fn map_admin_system_stats(
    row: sqlx::sqlite::SqliteRow,
) -> Result<AdminSystemStats, DataLayerError> {
    Ok(AdminSystemStats {
        total_users: row.try_get::<i64, _>("total_users").map_sql_err()?.max(0) as u64,
        active_users: row.try_get::<i64, _>("active_users").map_sql_err()?.max(0) as u64,
        total_api_keys: row
            .try_get::<i64, _>("total_api_keys")
            .map_sql_err()?
            .max(0) as u64,
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_sql_err()?
            .max(0) as u64,
    })
}
