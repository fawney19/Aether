use super::*;

const ACCOUNT_ATTRIBUTION: i64 = 20260917000000;
const DIRTY_EVENTS: i64 = 20260917000100;

async fn rows_snapshot(pool: &PgPool, table: &str) -> String {
    query_scalar(&format!(
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]')::text FROM {table} t"
    ))
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn legacy_overview_upgrade_preserves_applied_history_and_existing_statistics() {
    let Some(server) = ManagedPostgresServer::try_start().await.unwrap() else {
        return;
    };
    let mut connection = PgConnection::connect(server.database_url()).await.unwrap();
    connection.ensure_migrations_table().await.unwrap();
    for migration in POSTGRES_MIGRATOR.iter().filter(|migration| {
        migration.version <= 20260920120000 && migration.version != DIRTY_EVENTS
    }) {
        connection.apply(migration).await.unwrap();
    }
    connection.close().await.unwrap();
    let pool = PgPool::connect(server.database_url()).await.unwrap();

    // The restored September 17 migration installs the historical direct-write
    // trigger. Remove schema additions folded into the September 11 baseline
    // so this exercises an already-running database that never received them.
    sqlx::raw_sql(
        r#"
DROP TABLE public.stats_overview_dirty_events;
DROP INDEX public.ix_usage_attribution_owner_request;
UPDATE public.dashboard_stats_state SET stats_since = clock_timestamp() - INTERVAL '1 day';
INSERT INTO users(id, username, email_verified)
VALUES ('legacy-upgrade-owner', 'legacy-upgrade-owner', false);
INSERT INTO api_keys(id, user_id, key_hash)
VALUES ('legacy-upgrade-key', 'legacy-upgrade-owner', repeat('a', 64));
INSERT INTO usage(id, request_id, user_id, api_key_id, provider_name, model,
                  status, billing_status, created_at, response_time_ms)
VALUES ('legacy-upgrade-request', 'legacy-upgrade-request',
        'legacy-upgrade-owner', 'legacy-upgrade-key', 'test', 'test',
        'completed', 'settled', clock_timestamp() - INTERVAL '1 minute', 100);
INSERT INTO stats_overview_hourly(projection_version, bucket_start, dimensions, metrics)
SELECT projection_version, bucket_start, '{}'::jsonb, '{"request_count":1}'::jsonb
FROM stats_bucket_state WHERE granularity = 'hour';
INSERT INTO stats_overview_daily(projection_version, bucket_start, dimensions, metrics)
SELECT projection_version, bucket_start, '{}'::jsonb, '{"request_count":1}'::jsonb
FROM stats_bucket_state WHERE granularity = 'day';
UPDATE stats_bucket_state SET built_revision=source_revision, coverage_status='complete';
"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Model the old September 19 schema after creating real dashboard totals.
    // No fact is changed while its later retention helpers are absent.
    sqlx::raw_sql(
        r#"
DROP FUNCTION public.dashboard_ensure_activity_minute(timestamptz, smallint);
DROP TABLE public.dashboard_activity_minute;
ALTER TABLE public.dashboard_stats_state DROP COLUMN contributions_cleanup_cursor;
UPDATE _sqlx_migrations SET checksum=decode(
  '1dd622827b22ae0540f5e43232e7419727aa02d0742649af7e045185e9a13f68655ef7b31007bd0bf6e9e63177022815', 'hex')
WHERE version=20260911000000;
UPDATE _sqlx_migrations SET checksum=decode(
  'c64293c5d95fba6c3c43fff764a89da0225b386cfbef14743ab585e1db012fea35fe2cb2eee132e0fb5dac834c0410f4', 'hex')
WHERE version=20260919000000;
"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let historical_records = rows_snapshot(&pool, "_sqlx_migrations").await;
    let preserved_tables = [
        "users",
        "api_keys",
        "usage",
        "usage_attribution_snapshots",
        "stats_bucket_state",
        "stats_overview_hourly",
        "stats_overview_daily",
        "dashboard_request_contributions",
        "dashboard_stats_total",
        "dashboard_stats_minute",
        "dashboard_activity_hour",
        "dashboard_actor_minute",
    ];
    let mut original_rows = Vec::new();
    for table in preserved_tables {
        original_rows.push((table, rows_snapshot(&pool, table).await));
    }
    let account_record: String =
        query_scalar("SELECT to_jsonb(m)::text FROM _sqlx_migrations m WHERE version=$1")
            .bind(ACCOUNT_ATTRIBUTION)
            .fetch_one(&pool)
            .await
            .unwrap();
    let pending = prepare_database_for_startup(&pool).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>(),
        vec![
            DIRTY_EVENTS,
            20260921010000,
            20260921020000,
            20260921020100,
            20260923000000,
            20261001000000,
            20261004000000,
            20261007000000,
            20261008000000,
            20261009000000,
        ]
    );
    assert_eq!(
        rows_snapshot(&pool, "_sqlx_migrations").await,
        historical_records
    );

    for _ in 0..2 {
        super::super::run_migrations(&pool).await.unwrap();
        assert!(super::super::pending_migrations(&pool)
            .await
            .unwrap()
            .is_empty());
        for (table, expected) in &original_rows {
            assert_eq!(&rows_snapshot(&pool, table).await, expected, "{table}");
        }
        let after: String =
            query_scalar("SELECT to_jsonb(m)::text FROM _sqlx_migrations m WHERE version=$1")
                .bind(ACCOUNT_ATTRIBUTION)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            after, account_record,
            "the historical migration must not be restamped"
        );
    }
    assert!(table_exists(&pool, "stats_overview_dirty_events")
        .await
        .unwrap());
    assert!(table_exists(&pool, "dashboard_activity_minute")
        .await
        .unwrap());
    assert!(column_exists(
        &pool,
        "dashboard_stats_state",
        "contributions_cleanup_cursor"
    )
    .await
    .unwrap());
    for index in [
        "ix_usage_attribution_owner_request",
        "ix_usage_analytics_actor_metadata",
    ] {
        let valid: bool =
            query_scalar("SELECT indisvalid FROM pg_index WHERE indexrelid=to_regclass($1)")
                .bind(index)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(valid, "{index}");
    }
    let empty_projections: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM stats_overview_dirty_events), (SELECT count(*) FROM dashboard_activity_minute)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(empty_projections, (0, 0), "upgrade must not backfill usage");

    // Correcting an existing request must seed the retained activity counter
    // from its old detailed minute, without counting that request twice.
    query("UPDATE usage SET response_time_ms=200 WHERE request_id='legacy-upgrade-request'")
        .execute(&pool)
        .await
        .unwrap();
    let corrected: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT sum((metrics->>'request_count')::bigint)::bigint FROM dashboard_stats_total), (SELECT sum(request_count)::bigint FROM dashboard_activity_minute), (SELECT sum((metrics->>'response_sum_ms')::bigint)::bigint FROM dashboard_stats_total)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(corrected, (1, 1, 200));
    query(
        "INSERT INTO usage(id,request_id,user_id,api_key_id,provider_name,model,status,billing_status,created_at,response_time_ms) SELECT 'after-upgrade','after-upgrade',user_id,api_key_id,provider_name,model,status,billing_status,created_at,300 FROM usage WHERE request_id='legacy-upgrade-request'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let written: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT sum((metrics->>'request_count')::bigint)::bigint FROM dashboard_stats_total), (SELECT sum(request_count)::bigint FROM dashboard_activity_minute), (SELECT count(*) FROM stats_overview_dirty_events)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(written, (2, 2, 4));
    let repo = aether_data_postgres::SqlxUsageReadRepository::new(pool.clone());
    assert_eq!(
        repo.rebuild_overview_buckets(chrono::Utc::now() + chrono::Duration::days(1), 8)
            .await
            .unwrap(),
        2
    );
    let rebuilt: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT sum((metrics->>'request_count')::bigint)::bigint FROM stats_overview_hourly), (SELECT count(*) FROM stats_overview_dirty_events)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rebuilt, (2, 0));
    pool.close().await;
}
