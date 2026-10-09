use super::*;
use aether_data_contracts::repository::usage::{UsageAnalyticsQuery, UsageAnalyticsView};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;

#[tokio::test]
async fn dashboard_chart_history_coverage_matches_legacy_request_scope_without_raw_rescans() {
    let Some(server) = ManagedPostgresServer::try_start().await.unwrap() else {
        return;
    };
    let pool = PgPool::connect(server.database_url()).await.unwrap();
    prepare_and_apply_clean_postgres_database(&pool).await;
    let repo = aether_data_postgres::SqlxUsageReadRepository::new(pool.clone());
    let start = "2020-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    query("INSERT INTO stats_summary(id,cutoff_date) VALUES('history',$1)")
        .bind(start + Duration::days(4))
        .execute(&pool)
        .await
        .unwrap();
    for (day, count, complete) in [
        (0, 2, true),
        (1, 2, true),
        (2, 1, true),
        (3, 1, false),
        (4, 1, true),
    ] {
        query("INSERT INTO stats_daily(id,date,total_requests,total_cost,actual_total_cost,is_complete) VALUES($1,$2,$3,9999,7777,$4)")
            .bind(format!("day-{day}"))
            .bind(start + Duration::days(day))
            .bind(count)
            .bind(complete)
            .execute(&pool).await.unwrap();
    }
    for (id, day, status, provider, session) in [
        ("retained-request", 0, "completed", "provider", false),
        ("retained-session", 0, "completed", "provider", true),
        ("partial-request", 1, "completed", "provider", false),
        ("partial-pending", 1, "pending", "provider", false),
        ("partial-unknown", 1, "failed", "unknown", false),
    ] {
        query("INSERT INTO usage(id,request_id,model,provider_name,status,billing_status,total_cost_usd,actual_total_cost_usd,created_at,request_metadata) VALUES($1,$1,'model',$2,$3,'settled',2,1,$4,$5)")
            .bind(id).bind(provider).bind(status)
            .bind(start + Duration::days(day) + Duration::seconds(1))
            .bind(json!({"routing_group_billing_multiplier":2,"analytics_attribution":{"record_kind":if session { "session" } else { "request" }}}))
            .execute(&pool).await.unwrap();
    }
    query("UPDATE usage SET total_tokens=999,input_tokens=90,output_tokens=20,cache_creation_input_tokens=11,cache_read_input_tokens=13 WHERE id='retained-request'")
        .execute(&pool).await.unwrap();
    query("INSERT INTO usage_settlement_snapshots(request_id,billing_status,billing_effective_input_tokens,billing_output_tokens,billing_cache_creation_tokens,billing_cache_read_tokens) VALUES('retained-request','settled',7,3,5,2)")
        .execute(&pool).await.unwrap();
    let day_query = |day| UsageAnalyticsQuery {
        from_unix_ms: (start + Duration::days(day)).timestamp_millis() as u64,
        to_unix_ms: (start + Duration::days(day + 1)).timestamp_millis() as u64,
        timezone: "UTC".into(),
        view: UsageAnalyticsView::DashboardCharts,
        limit: 10_000,
        ..Default::default()
    };

    // A retained session belongs to the old rollup, but never to chart totals.
    let complete = repo.query_usage_analytics(&day_query(0)).await.unwrap();
    assert_eq!(complete.unrecoverable_bucket_count, 0);
    assert_eq!(complete.summary.request_count, 1);
    assert_eq!(complete.summary.total_tokens, 17);
    assert_eq!(
        complete.summary.billable_amount.as_deref(),
        Some("4.00000000")
    );
    let mut canonical_query = day_query(0);
    canonical_query.model = Some("model".into());
    let canonical = repo.query_usage_analytics(&canonical_query).await.unwrap();
    assert_eq!(
        complete.summary.total_tokens,
        canonical.summary.total_tokens
    );
    assert_eq!(
        complete.summary.billable_amount,
        canonical.summary.billable_amount
    );

    // Pending and unknown-provider rows cannot disguise a missing legacy request.
    let partial = repo.query_usage_analytics(&day_query(1)).await.unwrap();
    assert_eq!(partial.unrecoverable_bucket_count, 24);
    assert_eq!(partial.summary.request_count, 3);
    assert_eq!(
        partial.summary.billable_amount.as_deref(),
        Some("12.00000000")
    );

    let missing = repo.query_usage_analytics(&day_query(2)).await.unwrap();
    assert_eq!(missing.unrecoverable_bucket_count, 24);
    assert_eq!(missing.summary.request_count, 0);
    assert_eq!(missing.summary.billable_amount, None);
    assert_eq!(missing.rows[0].metrics.billable_amount, None);

    // Incomplete rollups, unpublished days and filtered views cannot establish
    // that request details are missing from global legacy daily totals.
    for day in [3, 4] {
        let result = repo.query_usage_analytics(&day_query(day)).await.unwrap();
        assert_eq!(result.unrecoverable_bucket_count, 0);
        assert_eq!(
            result.summary.billable_amount.as_deref(),
            Some("0.00000000")
        );
    }
    let mut filtered_query = day_query(2);
    filtered_query.model = Some("model".into());
    let filtered = repo.query_usage_analytics(&filtered_query).await.unwrap();
    assert_eq!(filtered.unrecoverable_bucket_count, 0);
    let mut partial_day_query = day_query(2);
    partial_day_query.from_unix_ms += 60 * 60 * 1000;
    let partial_day = repo
        .query_usage_analytics(&partial_day_query)
        .await
        .unwrap();
    assert_eq!(partial_day.unrecoverable_bucket_count, 23);
    assert_eq!(partial_day.summary.billable_amount, None);
    // A local day crosses two UTC archive days. Read both complete UTC days for
    // coverage, while chart totals remain bounded to the original local range.
    let local_day_query = UsageAnalyticsQuery {
        from_unix_ms: (start + Duration::days(1) + Duration::hours(16)).timestamp_millis() as u64,
        to_unix_ms: (start + Duration::days(2) + Duration::hours(16)).timestamp_millis() as u64,
        timezone: "Asia/Shanghai".into(),
        ..day_query(2)
    };
    let local_day = repo.query_usage_analytics(&local_day_query).await.unwrap();
    assert_eq!(local_day.unrecoverable_bucket_count, 24);
    assert_eq!(local_day.summary.request_count, 0);
    assert_eq!(local_day.rows.len(), 1);
    assert_eq!(local_day.rows[0].metrics.billable_amount, None);
    let mut retained_partial = day_query(0);
    retained_partial.from_unix_ms += 60 * 60 * 1000;
    let retained_partial = repo.query_usage_analytics(&retained_partial).await.unwrap();
    assert_eq!(retained_partial.unrecoverable_bucket_count, 0);
    assert_eq!(retained_partial.summary.request_count, 0);

    // Existing lost-hour evidence and the inferred day's coverage are one set.
    query("INSERT INTO stats_overview_dirty_events(transaction_id,projection_version,granularity,bucket_start,unrecoverable) VALUES(txid_current(),'overview-v2','hour',$1,true)")
        .bind(start + Duration::days(2) + Duration::hours(3))
        .execute(&pool).await.unwrap();
    let deduplicated = repo.query_usage_analytics(&day_query(2)).await.unwrap();
    assert_eq!(deduplicated.unrecoverable_bucket_count, 24);
    pool.close().await;
}
