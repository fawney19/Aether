use sqlx::{
    migrate::{AppliedMigration, Migrate, MigrateError, Migrator},
    SqlitePool,
};

use aether_data_contracts::PendingMigrationInfo;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}

pub async fn pending_migrations(
    pool: &SqlitePool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    let mut conn = pool.acquire().await?;
    let applied_migrations = match conn.list_applied_migrations().await {
        Ok(applied_migrations) => applied_migrations,
        Err(err) if is_missing_sqlx_migrations_table_error(&err) => {
            return Ok(pending_migrations_from_applied(&[]));
        }
        Err(err) => return Err(err),
    };
    if let Some(version) = conn.dirty_version().await? {
        return Err(MigrateError::Dirty(version));
    }
    validate_applied_migrations(&applied_migrations)?;
    Ok(pending_migrations_from_applied(&applied_migrations))
}

pub async fn prepare_database_for_startup(
    pool: &SqlitePool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    pending_migrations(pool).await
}

fn is_missing_sqlx_migrations_table_error(err: &MigrateError) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("_sqlx_migrations")
        && (message.contains("no such table")
            || message.contains("doesn't exist")
            || message.contains("does not exist")
            || message.contains("unknown table"))
}

fn pending_migrations_from_applied(
    applied_migrations: &[sqlx::migrate::AppliedMigration],
) -> Vec<PendingMigrationInfo> {
    let applied_versions = applied_migrations
        .iter()
        .map(|migration| migration.version)
        .collect::<std::collections::HashSet<_>>();
    MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| !applied_versions.contains(&migration.version))
        .map(|migration| PendingMigrationInfo {
            version: migration.version,
            description: migration.description.to_string(),
        })
        .collect()
}

fn validate_applied_migrations(
    applied_migrations: &[AppliedMigration],
) -> Result<(), MigrateError> {
    if MIGRATOR.ignore_missing {
        return Ok(());
    }
    let known_versions = MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .collect::<std::collections::HashSet<_>>();
    if let Some(migration) = applied_migrations
        .iter()
        .find(|migration| !known_versions.contains(&migration.version))
    {
        return Err(MigrateError::VersionMissing(migration.version));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{
        pending_migrations, prepare_database_for_startup, run_migrations,
        validate_applied_migrations, MIGRATOR,
    };
    use sqlx::migrate::{AppliedMigration, MigrateError};

    #[tokio::test]
    async fn migrates_empty_database_and_clears_pending_set() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");

        let pending = pending_migrations(&pool).await.expect("pending migrations");
        assert_eq!(pending.len(), MIGRATOR.iter().count());
        assert!(!pending.is_empty());

        run_migrations(&pool).await.expect("run sqlite migrations");
        assert!(pending_migrations(&pool)
            .await
            .expect("pending migrations after run")
            .is_empty());
    }

    #[test]
    fn rejects_applied_migration_versions_unknown_to_this_binary() {
        let version = MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .max()
            .expect("sqlite migrations should not be empty")
            + 1;
        let error = validate_applied_migrations(&[AppliedMigration {
            version,
            checksum: Cow::Borrowed(&[]),
        }])
        .expect_err("unknown applied migration should block startup");

        assert!(matches!(error, MigrateError::VersionMissing(found) if found == version));
    }

    #[test]
    fn embeds_aggregate_id_width_migration() {
        assert!(MIGRATOR
            .iter()
            .any(|migration| migration.version == 20260825000000));
    }

    #[tokio::test]
    async fn migrates_cross_driver_schema_parity_contract() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");

        run_migrations(&pool).await.expect("run sqlite migrations");
        assert!(MIGRATOR
            .iter()
            .any(|migration| migration.version == 20260725010000));

        let parity_table_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM sqlite_master
WHERE type = 'table'
  AND name IN (
    'api_key_provider_mappings',
    'provider_usage_tracking',
    'stats_summary',
    'user_model_usage_counts',
    'usage_body_blobs',
    'usage_http_audits'
  )
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("parity tables should be inspectable");
        assert_eq!(parity_table_count, 6);

        let usage_column_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM pragma_table_info('usage')
WHERE name IN (
  'input_output_total_tokens',
  'cache_creation_input_tokens_5m',
  'cache_creation_input_tokens_1h',
  'input_context_tokens',
  'input_cost_usd',
  'output_cost_usd',
  'cache_cost_usd',
  'cache_creation_cost_usd_5m',
  'cache_creation_cost_usd_1h',
  'request_cost_usd',
  'actual_input_cost_usd',
  'actual_output_cost_usd',
  'actual_cache_cost_usd',
  'actual_cache_creation_cost_usd',
  'actual_cache_creation_cost_usd_5m',
  'actual_cache_creation_cost_usd_1h',
  'actual_cache_read_cost_usd',
  'actual_request_cost_usd',
  'rate_multiplier',
  'input_price_per_1m',
  'cache_creation_price_per_1m',
  'cache_creation_price_per_1m_5m',
  'cache_creation_price_per_1m_1h',
  'cache_read_price_per_1m',
  'price_per_request',
  'request_headers',
  'request_body',
  'provider_request_headers',
  'provider_request_body',
  'response_headers',
  'response_body',
  'client_response_headers',
  'client_response_body',
  'request_body_compressed',
  'provider_request_body_compressed',
  'response_body_compressed',
  'client_response_body_compressed',
  'created_at',
  'username',
  'api_key_name'
)
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("usage parity columns should be inspectable");
        assert_eq!(usage_column_count, 40);

        let settlement_column_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM pragma_table_info('usage_settlement_snapshots')
WHERE name IN (
  'billing_snapshot_schema_version',
  'billing_snapshot_status',
  'rate_multiplier',
  'is_free_tier',
  'input_price_per_1m',
  'output_price_per_1m',
  'cache_creation_price_per_1m',
  'cache_read_price_per_1m',
  'price_per_request',
  'settlement_snapshot_schema_version',
  'settlement_snapshot',
  'billing_dimensions',
  'billing_input_tokens',
  'billing_effective_input_tokens',
  'billing_output_tokens',
  'billing_cache_creation_tokens',
  'billing_cache_creation_5m_tokens',
  'billing_cache_creation_1h_tokens',
  'billing_cache_read_tokens',
  'billing_total_input_context',
  'billing_cache_creation_cost_usd',
  'billing_cache_read_cost_usd',
  'billing_total_cost_usd',
  'billing_actual_total_cost_usd',
  'billing_pricing_source',
  'billing_rule_id',
  'billing_rule_version'
)
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("Billing V3 columns should be inspectable");
        assert_eq!(settlement_column_count, 27);

        let catalog_video_stats_column_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM (
  SELECT name FROM pragma_table_info('provider_api_keys')
  WHERE name IN ('last_error_at', 'last_error_msg')
  UNION ALL
  SELECT name FROM pragma_table_info('video_tasks')
  WHERE name IN (
    'converted_request_body', 'max_retries', 'video_urls', 'thumbnail_url',
    'video_size_bytes', 'video_expires_at', 'stored_video_path', 'storage_provider',
    'remixed_from_task_id', 'webhook_url', 'webhook_sent', 'webhook_sent_at',
    'video_duration_seconds'
  )
  UNION ALL
  SELECT name FROM pragma_table_info('stats_daily')
  WHERE name IN (
    'input_cost', 'output_cost', 'cache_creation_cost', 'cache_read_cost',
    'p50_response_time_ms', 'p90_response_time_ms', 'p99_response_time_ms',
    'p50_first_byte_time_ms', 'p90_first_byte_time_ms', 'p99_first_byte_time_ms'
  )
)
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("catalog, video, and stats parity columns should be inspectable");
        assert_eq!(catalog_video_stats_column_count, 25);

        let parity_index_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM sqlite_master
WHERE type = 'index'
  AND name IN (
    'ix_usage_body_blobs_request_id',
    'ix_usage_http_audits_updated_at',
    'ix_usage_settlement_snapshots_schema_version',
    'ix_usage_settlement_snapshots_pricing_source',
    'idx_usage_stale_pending_created_request',
    'idx_provider_api_keys_provider_created_at_desc',
    'idx_provider_api_keys_provider_last_used_at_desc'
  )
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("parity indexes should be inspectable");
        assert_eq!(parity_index_count, 7);
    }

    #[tokio::test]
    async fn migrates_advanced_stats_parity_contract() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");

        run_migrations(&pool).await.expect("run sqlite migrations");
        assert!(MIGRATOR
            .iter()
            .any(|migration| migration.version == 20260725020000));

        let table_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM sqlite_master
WHERE type = 'table'
  AND name IN (
    'stats_user_summary',
    'stats_user_daily_api_format',
    'stats_user_daily_model',
    'stats_user_daily_provider',
    'stats_user_daily_model_provider',
    'stats_daily_model_provider',
    'stats_daily_cost_savings',
    'stats_daily_cost_savings_provider',
    'stats_daily_cost_savings_model',
    'stats_daily_cost_savings_model_provider',
    'stats_user_daily_cost_savings',
    'stats_user_daily_cost_savings_provider',
    'stats_user_daily_cost_savings_model',
    'stats_user_daily_cost_savings_model_provider'
  )
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("advanced stats tables should be inspectable");
        assert_eq!(table_count, 14);

        let enriched_column_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM (
  SELECT name FROM pragma_table_info('stats_daily')
  WHERE name IN (
    'effective_input_tokens', 'total_input_context', 'response_time_sum_ms',
    'response_time_samples', 'cache_hit_total_requests', 'cache_hit_requests',
    'completed_total_input_context', 'settled_total_cost',
    'settled_first_finalized_at_unix_secs'
  )
  UNION ALL
  SELECT name FROM pragma_table_info('stats_hourly')
  WHERE name IN (
    'response_time_sum_ms', 'response_time_samples', 'cache_hit_total_requests',
    'completed_total_input_context', 'settled_total_cost'
  )
  UNION ALL
  SELECT name FROM pragma_table_info('stats_user_daily')
  WHERE name IN (
    'effective_input_tokens', 'total_input_context', 'actual_total_cost',
    'response_time_samples', 'settled_total_cost'
  )
)
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("advanced stats columns should be inspectable");
        assert_eq!(enriched_column_count, 19);
    }

    #[tokio::test]
    async fn advanced_stats_migration_invalidates_completed_legacy_buckets() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");

        for migration in MIGRATOR
            .iter()
            .filter(|migration| migration.version < 20260725020000)
        {
            sqlx::raw_sql(migration.sql.as_ref())
                .execute(&pool)
                .await
                .unwrap_or_else(|err| panic!("migration {} should run: {err}", migration.version));
        }

        sqlx::query(
            r#"
INSERT INTO stats_hourly (id, hour_utc, is_complete, created_at, updated_at)
VALUES ('legacy-hour', 3600, 1, 1, 1)
"#,
        )
        .execute(&pool)
        .await
        .expect("legacy hourly bucket should seed");
        sqlx::query(
            r#"
INSERT INTO stats_daily (id, "date", is_complete, created_at, updated_at)
VALUES ('legacy-day', 86400, 1, 1, 1)
"#,
        )
        .execute(&pool)
        .await
        .expect("legacy daily bucket should seed");

        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260725020000)
            .expect("advanced stats migration should be embedded");
        sqlx::raw_sql(migration.sql.as_ref())
            .execute(&pool)
            .await
            .expect("advanced stats migration should run");

        let hourly_complete: i64 =
            sqlx::query_scalar("SELECT is_complete FROM stats_hourly WHERE id = 'legacy-hour'")
                .fetch_one(&pool)
                .await
                .expect("hourly completion state should load");
        let daily_complete: i64 =
            sqlx::query_scalar("SELECT is_complete FROM stats_daily WHERE id = 'legacy-day'")
                .fetch_one(&pool)
                .await
                .expect("daily completion state should load");
        assert_eq!((hourly_complete, daily_complete), (0, 0));
    }

    #[tokio::test]
    async fn routing_snapshot_migration_backfills_legacy_usage_columns() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");

        for migration in MIGRATOR
            .iter()
            .filter(|migration| migration.version < 20260725030000)
        {
            sqlx::raw_sql(migration.sql.as_ref())
                .execute(&pool)
                .await
                .unwrap_or_else(|err| panic!("migration {} should run: {err}", migration.version));
        }

        sqlx::query(
            r#"
INSERT INTO "usage" (
  request_id, provider_name, model, provider_id, provider_endpoint_id,
  provider_api_key_id, candidate_id, candidate_index, route_family,
  has_format_conversion, created_at_unix_ms, updated_at_unix_secs
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind("routing-backfill-request")
        .bind("provider")
        .bind("model")
        .bind("provider-1")
        .bind("endpoint-1")
        .bind("provider-key-1")
        .bind("candidate-1")
        .bind(7_i64)
        .bind("direct")
        .bind(true)
        .bind(111_i64)
        .bind(222_i64)
        .execute(&pool)
        .await
        .expect("legacy routing usage should insert");

        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260725030000)
            .expect("routing snapshot migration should be embedded");
        sqlx::raw_sql(migration.sql.as_ref())
            .execute(&pool)
            .await
            .expect("routing snapshot migration should run");

        let row = sqlx::query_as::<
            _,
            (
                String,
                Option<i64>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<bool>,
                i64,
                i64,
            ),
        >(
            r#"
SELECT candidate_id, candidate_index, route_family, selected_provider_id,
       selected_provider_api_key_id, has_format_conversion, created_at, updated_at
FROM usage_routing_snapshots
WHERE request_id = ?
"#,
        )
        .bind("routing-backfill-request")
        .fetch_one(&pool)
        .await
        .expect("backfilled routing snapshot should load");
        assert_eq!(
            row,
            (
                "candidate-1".to_string(),
                Some(7),
                Some("direct".to_string()),
                Some("provider-1".to_string()),
                Some("provider-key-1".to_string()),
                Some(true),
                111,
                222,
            )
        );

        sqlx::query("DELETE FROM \"usage\" WHERE request_id = ?")
            .bind("routing-backfill-request")
            .execute(&pool)
            .await
            .expect("usage row should delete");
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_routing_snapshots")
            .fetch_one(&pool)
            .await
            .expect("routing snapshot count should load");
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn worker_boot_cleanup_removes_only_legacy_instance_rows_and_events() {
        const CLEANUP_VERSION: i64 = 20260731000000;
        const HASHED_LEGACY_ID: &str =
            "boot:maintenance.request.candidate.cleanup:~0123456789abcdef0123";
        const OVERLONG_LEGACY_ID: &str =
            "boot:maintenance.proxy.node.metrics.cleanup:gateway-instance-with-an-overlong-id";

        assert_eq!(HASHED_LEGACY_ID.len(), 64);
        assert!(OVERLONG_LEGACY_ID.len() > 64);

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");

        for migration in MIGRATOR
            .iter()
            .filter(|migration| migration.version < CLEANUP_VERSION)
        {
            sqlx::raw_sql(migration.sql.as_ref())
                .execute(&pool)
                .await
                .unwrap_or_else(|err| panic!("migration {} should run: {err}", migration.version));
        }

        sqlx::query(
            r#"
INSERT INTO background_task_runs (
  id, task_key, kind, "trigger", status, owner_instance,
  progress_message, created_by, created_at_unix_secs, updated_at_unix_secs
) VALUES
  ('boot:usage.queue.worker:gateway-a', 'usage.queue.worker', 'daemon', 'daemon',
   'running', 'gateway-a', 'worker booted', 'system', 1, 1),
  ('boot:usage.queue.worker:gateway-b', 'usage.queue.worker', 'daemon', 'daemon',
   'running', 'gateway-b', 'worker booted', 'system', 2, 2),
  ('boot:maintenance.request.candidate.cleanup:~0123456789abcdef0123',
   'maintenance.request.candidate.cleanup', 'scheduled', 'interval',
   'running', 'gateway-hash', 'worker booted', 'system', 3, 3),
  ('boot:maintenance.proxy.node.metrics.cleanup:gateway-instance-with-an-overlong-id',
   'maintenance.proxy.node.metrics.cleanup', 'scheduled', 'interval',
   'running', 'gateway-overlong', 'worker booted', 'system', 4, 4),
  ('boot:model.fetch.worker', 'model.fetch.worker', 'scheduled', 'interval',
   'running', 'gateway-early-fix', 'worker booted', 'system', 5, 5),
  ('boot:usage.queue.worker', 'usage.queue.worker', 'daemon', 'daemon',
   'running', NULL, 'worker registered', 'system', 6, 6),
  ('boot:ownerless-worker', 'ownerless.worker', 'daemon', 'daemon',
   'running', NULL, 'worker booted', 'system', 7, 7),
  ('boot:custom-progress', 'custom.progress', 'daemon', 'daemon',
   'running', 'gateway-custom', 'worker healthy', 'system', 8, 8),
  ('boot:user-request', 'user.request', 'on_demand', 'manual',
   'running', 'gateway-user', 'worker booted', 'admin', 9, 9);

INSERT INTO background_task_events (
  id, run_id, event_type, message, created_at_unix_secs
) VALUES
  ('legacy-event-a', 'boot:usage.queue.worker:gateway-a', 'worker_boot', 'legacy', 1),
  ('legacy-event-b', 'boot:usage.queue.worker:gateway-b', 'worker_boot', 'legacy', 2),
  ('legacy-event-hash', 'boot:maintenance.request.candidate.cleanup:~0123456789abcdef0123',
   'worker_boot', 'legacy hash', 3),
  ('legacy-event-overlong',
   'boot:maintenance.proxy.node.metrics.cleanup:gateway-instance-with-an-overlong-id',
   'worker_boot', 'legacy overlong', 4),
  ('early-fix-event', 'boot:model.fetch.worker', 'worker_boot', 'early fix', 5),
  ('logical-event', 'boot:usage.queue.worker', 'worker_boot', 'logical', 6),
  ('ownerless-event', 'boot:ownerless-worker', 'worker_boot', 'ownerless', 7),
  ('custom-progress-event', 'boot:custom-progress', 'worker_boot', 'custom progress', 8),
  ('manual-event', 'boot:user-request', 'manual', 'manual', 9);
"#,
        )
        .execute(&pool)
        .await
        .expect("worker boot cleanup fixtures should insert");

        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == CLEANUP_VERSION)
            .expect("worker boot cleanup migration should be embedded");
        sqlx::raw_sql(migration.sql.as_ref())
            .execute(&pool)
            .await
            .expect("worker boot cleanup migration should run");
        sqlx::raw_sql(migration.sql.as_ref())
            .execute(&pool)
            .await
            .expect("worker boot cleanup migration should be idempotent");

        let remaining_runs = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            r#"
SELECT id, owner_instance, created_by
FROM background_task_runs
ORDER BY id
"#,
        )
        .fetch_all(&pool)
        .await
        .expect("remaining worker task rows should load");
        assert_eq!(
            remaining_runs,
            vec![
                (
                    "boot:custom-progress".to_string(),
                    Some("gateway-custom".to_string()),
                    Some("system".to_string()),
                ),
                (
                    "boot:ownerless-worker".to_string(),
                    None,
                    Some("system".to_string()),
                ),
                (
                    "boot:usage.queue.worker".to_string(),
                    None,
                    Some("system".to_string()),
                ),
                (
                    "boot:user-request".to_string(),
                    Some("gateway-user".to_string()),
                    Some("admin".to_string()),
                ),
            ]
        );

        let remaining_events =
            sqlx::query_scalar::<_, String>("SELECT id FROM background_task_events ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("remaining worker task events should load");
        assert_eq!(
            remaining_events,
            vec![
                "custom-progress-event".to_string(),
                "logical-event".to_string(),
                "manual-event".to_string(),
                "ownerless-event".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn codex_live_permission_migration_is_scoped_and_idempotent() {
        const MIGRATION_VERSION: i64 = 20260821000000;

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");
        run_migrations(&pool).await.expect("run sqlite migrations");

        sqlx::raw_sql(
            r#"
INSERT INTO users (
  id, username, allowed_api_formats, allowed_api_formats_mode, created_at, updated_at
) VALUES
  ('user-specific', 'user-specific', '["openai:responses"]', 'specific', 1, 1),
  ('user-unrestricted', 'user-unrestricted', '["openai:responses"]', 'unrestricted', 1, 1);

INSERT INTO api_keys (
  id, user_id, key_hash, allowed_api_formats, created_at, updated_at
) VALUES (
  'api-key-specific', 'user-specific', 'api-key-specific-hash',
  '["openai:responses"]', 1, 1
);

INSERT INTO user_groups (
  id, name, normalized_name, allowed_api_formats, allowed_api_formats_mode,
  created_at, updated_at
) VALUES (
  'group-specific', 'Specific', 'specific', '["openai:responses"]', 'specific', 1, 1
);

INSERT INTO providers (id, name, provider_type, created_at, updated_at) VALUES
  ('provider-codex', 'Codex migration fixture', 'codex', 1, 1),
  ('provider-openai', 'OpenAI migration fixture', 'openai', 1, 1);

INSERT INTO provider_api_keys (
  id, provider_id, name, api_formats, auth_type_by_format,
  allow_auth_channel_mismatch_formats, rate_multipliers,
  global_priority_by_format, created_at, updated_at
) VALUES
  (
    'provider-key-codex', 'provider-codex', 'Codex key',
    '["openai:responses"]', '{"openai:responses":"oauth"}',
    '["openai:responses"]', '{"openai:responses":1.25}',
    '{"openai:responses":17}', 1, 1
  ),
  (
    'provider-key-openai', 'provider-openai', 'OpenAI key',
    '["openai:responses"]', '{"openai:responses":"api_key"}',
    '["openai:responses"]', '{"openai:responses":2.0}',
    '{"openai:responses":23}', 1, 1
  );
"#,
        )
        .execute(&pool)
        .await
        .expect("legacy Codex Live fixtures should insert");

        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == MIGRATION_VERSION)
            .expect("Codex Live permission migration should be embedded");
        for _ in 0..2 {
            sqlx::raw_sql(migration.sql.as_ref())
                .execute(&pool)
                .await
                .expect("Codex Live permission migration should run idempotently");
        }

        let user_specific: String =
            sqlx::query_scalar("SELECT allowed_api_formats FROM users WHERE id = 'user-specific'")
                .fetch_one(&pool)
                .await
                .expect("specific user formats should load");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&user_specific).unwrap(),
            serde_json::json!(["openai:responses", "codex:live"])
        );

        let user_unrestricted: String = sqlx::query_scalar(
            "SELECT allowed_api_formats FROM users WHERE id = 'user-unrestricted'",
        )
        .fetch_one(&pool)
        .await
        .expect("unrestricted user formats should load");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&user_unrestricted).unwrap(),
            serde_json::json!(["openai:responses"])
        );

        for (table, id_column, id) in [
            ("api_keys", "id", "api-key-specific"),
            ("user_groups", "id", "group-specific"),
        ] {
            let sql = format!("SELECT allowed_api_formats FROM {table} WHERE {id_column} = ?");
            let formats: String = sqlx::query_scalar(sql.as_str())
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("migrated downstream formats should load");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&formats).unwrap(),
                serde_json::json!(["openai:responses", "codex:live"])
            );
        }

        let codex_key = sqlx::query_as::<_, (String, String, String, String, String)>(
            r#"
SELECT api_formats, auth_type_by_format, allow_auth_channel_mismatch_formats,
       rate_multipliers, global_priority_by_format
FROM provider_api_keys
WHERE id = 'provider-key-codex'
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("migrated Codex key should load");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&codex_key.0).unwrap(),
            serde_json::json!(["openai:responses", "codex:live"])
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&codex_key.1).unwrap(),
            serde_json::json!({"openai:responses": "oauth", "codex:live": "oauth"})
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&codex_key.2).unwrap(),
            serde_json::json!(["openai:responses", "codex:live"])
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&codex_key.3).unwrap(),
            serde_json::json!({"openai:responses": 1.25, "codex:live": 1.25})
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&codex_key.4).unwrap(),
            serde_json::json!({"openai:responses": 17, "codex:live": 17})
        );

        let openai_formats: String = sqlx::query_scalar(
            "SELECT api_formats FROM provider_api_keys WHERE id = 'provider-key-openai'",
        )
        .fetch_one(&pool)
        .await
        .expect("non-Codex key formats should load");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&openai_formats).unwrap(),
            serde_json::json!(["openai:responses"])
        );
    }

    #[tokio::test]
    async fn pending_and_startup_preparation_reject_dirty_migration_state() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");
        run_migrations(&pool).await.expect("run sqlite migrations");

        let dirty_version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .expect("latest sqlite migration version should load");
        sqlx::query("UPDATE _sqlx_migrations SET success = FALSE WHERE version = ?")
            .bind(dirty_version)
            .execute(&pool)
            .await
            .expect("sqlite migration should be marked dirty");

        let pending_error = pending_migrations(&pool)
            .await
            .expect_err("dirty sqlite migration should fail pending inspection");
        assert!(
            matches!(&pending_error, MigrateError::Dirty(version) if *version == dirty_version),
            "unexpected pending migration error: {pending_error}"
        );

        let preparation_error = prepare_database_for_startup(&pool)
            .await
            .expect_err("dirty sqlite migration should fail startup preparation");
        assert!(
            matches!(&preparation_error, MigrateError::Dirty(version) if *version == dirty_version),
            "unexpected startup preparation error: {preparation_error}"
        );
    }
}
