use super::*;
use aether_data_contracts::repository::usage::{
    UsageAnalyticsQuery, UsageAnalyticsView, UsageDashboardAnalyticsQuery,
};
use chrono::{Duration, Utc};
use serde_json::json;

#[tokio::test]
async fn dashboard_charts_match_summary_customer_charges_and_request_scope() {
    let Some(server) = ManagedPostgresServer::try_start().await.unwrap() else {
        return;
    };
    let pool = PgPool::connect(server.database_url()).await.unwrap();
    prepare_and_apply_clean_postgres_database(&pool).await;
    let repo = aether_data_postgres::SqlxUsageReadRepository::new(pool.clone());
    let summary_query = UsageDashboardAnalyticsQuery {
        timezone: "Asia/Shanghai".into(),
    };
    let start = summary_query.today_start(Utc::now()).unwrap();
    query("UPDATE dashboard_stats_state SET stats_since=$1")
        .bind(start)
        .execute(&pool)
        .await
        .unwrap();
    let composite = json!({"billing_multiplier_snapshot":{"version":1,"factors":{"routing_group":2,"user_group":0.25},"multiplier":0.5}});
    for (id, provider, status, billing, base, actual, metadata) in [
        (
            "composite",
            "alpha",
            "completed",
            "settled",
            10.0,
            Some(3.0),
            composite,
        ),
        (
            "legacy",
            "alpha",
            "completed",
            "settled",
            2.0,
            Some(1.25),
            json!({}),
        ),
        (
            "free",
            "alpha",
            "completed",
            "settled",
            50.0,
            Some(40.0),
            json!({"routing_group_billing_multiplier":0}),
        ),
        (
            "invalid",
            "alpha",
            "completed",
            "settled",
            99.0,
            Some(90.0),
            json!({"billing_multiplier_snapshot":null}),
        ),
        (
            "unpriced",
            "beta",
            "completed",
            "settled",
            80.0,
            Some(70.0),
            json!({"usage_pricing_available":false}),
        ),
        (
            "failed",
            "unknown",
            "failed",
            "settled",
            0.0,
            Some(0.0),
            json!({}),
        ),
        (
            "pending",
            "pending",
            "pending",
            "pending",
            0.0,
            None,
            json!({}),
        ),
        (
            "streaming",
            "beta",
            "streaming",
            "pending",
            0.0,
            None,
            json!({}),
        ),
        (
            "session",
            "alpha",
            "completed",
            "settled",
            500.0,
            Some(500.0),
            json!({}),
        ),
    ] {
        query("INSERT INTO usage(id,request_id,model,provider_name,status,billing_status,total_cost_usd,actual_total_cost_usd,total_tokens,created_at,request_metadata,response_time_ms) VALUES($1,$1,'model',$2,$3,$4,$5,$6,10,$7,$8,1000)")
            .bind(id).bind(provider).bind(status).bind(billing).bind(base).bind(actual)
            .bind(start + Duration::seconds(1)).bind(metadata).execute(&pool).await.unwrap();
    }
    // The captured settlement cost, rather than the mutable audit float, is rated.
    query("INSERT INTO usage_settlement_snapshots(request_id,billing_status,billing_total_cost_usd,billing_actual_total_cost_usd) VALUES('composite','settled',12,4)")
        .execute(&pool).await.unwrap();
    query(
        "UPDATE usage_attribution_snapshots SET record_kind='session' WHERE request_id='session'",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Exclude both the previous local day and the next day's boundary.
    for (id, at) in [
        ("before", start - Duration::seconds(1)),
        ("after", start + Duration::days(1)),
    ] {
        query("INSERT INTO usage(id,request_id,model,provider_name,status,billing_status,total_cost_usd,actual_total_cost_usd,created_at) VALUES($1,$1,'outside','outside','completed','settled',999,999,$2)")
            .bind(id).bind(at).execute(&pool).await.unwrap();
    }
    let summary = repo.query_dashboard_summary(&summary_query).await.unwrap();
    let charts = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            from_unix_ms: start.timestamp_millis() as u64,
            to_unix_ms: (start + Duration::days(1)).timestamp_millis() as u64,
            timezone: summary_query.timezone.clone(),
            view: UsageAnalyticsView::DashboardCharts,
            limit: 10_000,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(summary.today.request_count, 8);
    assert_eq!(summary.today.billable_amount.as_deref(), Some("7.25000000"));
    assert_eq!(charts.summary.request_count, summary.today.request_count);
    assert_eq!(
        charts.summary.billable_amount,
        summary.today.billable_amount
    );
    assert_eq!(
        charts.summary.pricing_available_count,
        summary.today.pricing_available_count
    );
    assert_eq!(charts.summary.in_flight_request_count, 2);
    assert_eq!(charts.rows.len(), 1);
    assert_eq!(charts.rows[0].metrics.request_count, 8);
    assert_eq!(
        charts.rows[0].metrics.billable_amount,
        summary.today.billable_amount
    );
    assert_eq!(charts.rows[0].metrics.unique_providers, Some(2));
    assert_eq!(
        charts.rows[0].bucket_start.as_deref(),
        Some(
            start
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                .as_str()
        )
    );
    for rows in [&charts.model_rows, &charts.provider_rows] {
        assert_eq!(rows.iter().map(|r| r.metrics.request_count).sum::<u64>(), 8);
        let charges: f64 = rows
            .iter()
            .filter_map(|r| r.metrics.billable_amount.as_ref())
            .map(|v| v.parse::<f64>().unwrap())
            .sum();
        assert_eq!(charges, 7.25);
    }
    assert_eq!(
        charts.provider_rows.len(),
        4,
        "legacy provider names must not collapse into one null-ID group"
    );
    // Retained older requests can have no contribution ledger entry. Mixing
    // fallback rows with current cached rows must keep unknown pricing distinct
    // from free usage and preserve the immutable composite settlement amount.
    query("DELETE FROM dashboard_request_contributions WHERE request_id IN ('composite','free','invalid')")
        .execute(&pool).await.unwrap();
    let mixed = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            from_unix_ms: start.timestamp_millis() as u64,
            to_unix_ms: (start + Duration::days(1)).timestamp_millis() as u64,
            timezone: summary_query.timezone.clone(),
            view: UsageAnalyticsView::DashboardCharts,
            limit: 10_000,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        mixed.summary.billable_amount,
        charts.summary.billable_amount
    );
    assert_eq!(mixed.summary.total_tokens, charts.summary.total_tokens);
    assert_eq!(mixed.summary.request_count, charts.summary.request_count);
    assert_eq!(
        mixed.summary.pricing_available_count,
        charts.summary.pricing_available_count
    );
    assert_eq!(mixed.rows.len(), charts.rows.len());
    for (mixed, cached) in mixed.rows.iter().zip(&charts.rows) {
        assert_eq!(mixed.bucket_start, cached.bucket_start);
        assert_eq!(
            mixed.metrics.billable_amount,
            cached.metrics.billable_amount
        );
        assert_eq!(mixed.metrics.total_tokens, cached.metrics.total_tokens);
        assert_eq!(
            mixed.metrics.pricing_available_count,
            cached.metrics.pricing_available_count
        );
    }
    let empty = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            from_unix_ms: (start - Duration::days(2)).timestamp_millis() as u64,
            to_unix_ms: (start - Duration::days(1)).timestamp_millis() as u64,
            timezone: summary_query.timezone,
            view: UsageAnalyticsView::DashboardCharts,
            limit: 10_000,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(empty.summary.request_count, 0);
    assert_eq!(empty.summary.billable_amount.as_deref(), Some("0.00000000"));
    assert_eq!(empty.rows[0].metrics.unique_providers, Some(0));
    pool.close().await;
}

#[tokio::test]
async fn customer_billing_parallel_fix_preserves_history_without_backfill() {
    let Some(server) = ManagedPostgresServer::try_start().await.unwrap() else {
        return;
    };
    let mut connection = PgConnection::connect(server.database_url()).await.unwrap();
    connection.ensure_migrations_table().await.unwrap();
    for migration in POSTGRES_MIGRATOR
        .iter()
        .filter(|m| m.version < 20261008000000)
    {
        connection.apply(migration).await.unwrap();
    }
    let pool = PgPool::connect(server.database_url()).await.unwrap();
    query("INSERT INTO stats_daily(id,date,total_requests,total_cost,actual_total_cost,is_complete) VALUES('untouched','2020-01-01',1,10,3,true)")
        .execute(&pool).await.unwrap();
    query("INSERT INTO usage(id,request_id,model,provider_name,status,billing_status,total_cost_usd,actual_total_cost_usd,created_at,request_metadata) VALUES('untouched','untouched','m','p','completed','settled',10,3,'2020-01-01','{\"routing_group_billing_multiplier\":2}')")
        .execute(&pool).await.unwrap();
    let before: (serde_json::Value,serde_json::Value) = sqlx::query_as("SELECT (SELECT to_jsonb(d) FROM stats_daily d WHERE id='untouched'), (SELECT to_jsonb(u) FROM usage u WHERE id='untouched')").fetch_one(&pool).await.unwrap();
    connection
        .apply(
            POSTGRES_MIGRATOR
                .iter()
                .find(|m| m.version == 20261008000000)
                .unwrap(),
        )
        .await
        .unwrap();
    let after = sqlx::query_as("SELECT (SELECT to_jsonb(d) FROM stats_daily d WHERE id='untouched'), (SELECT to_jsonb(u) FROM usage u WHERE id='untouched')").fetch_one(&pool).await.unwrap();
    assert_eq!(before, after);
    let parallel: String=query_scalar("SELECT proparallel::text FROM pg_proc WHERE oid='public.usage_customer_billable_amount(jsonb,numeric,numeric)'::regprocedure").fetch_one(&pool).await.unwrap();
    assert_eq!(parallel, "u");
    // Encourage a parallel scan; the exception-handling function must keep it serial.
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql("CREATE TABLE billing_parallel_probe AS SELECT i::numeric AS cost FROM generate_series(1,10000) i; ALTER TABLE billing_parallel_probe SET (parallel_workers=2); ANALYZE billing_parallel_probe; SET LOCAL min_parallel_table_scan_size=0; SET LOCAL parallel_setup_cost=0; SET LOCAL parallel_tuple_cost=0;").execute(&mut *tx).await.unwrap();
    let charge:String=query_scalar("SELECT sum(public.usage_customer_billable_amount('{\"routing_group_billing_multiplier\":2}'::jsonb,cost,1))::text FROM billing_parallel_probe").fetch_one(&mut *tx).await.unwrap();
    assert_eq!(charge, "100010000.00000000");
    tx.rollback().await.unwrap();
    pool.close().await;
}
