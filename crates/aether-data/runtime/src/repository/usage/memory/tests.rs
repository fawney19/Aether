use std::sync::Arc;

use super::InMemoryUsageReadRepository;
use crate::repository::auth::{
    AuthApiKeyReadRepository, InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
    StoredAuthApiKeySnapshot,
};
use crate::repository::provider_catalog::{
    InMemoryProviderCatalogReadRepository, ProviderCatalogReadRepository,
    ProviderCatalogWriteRepository, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use crate::repository::usage::{
    StoredProviderUsageWindow, StoredRequestUsageAudit, UpsertUsageRecord, UsageReadRepository,
    UsageWriteRepository,
};
use aether_data_contracts::repository::usage::{
    usage_body_ref, ProviderApiKeyWindowUsageRequest, UsageAuditAggregationGroupBy,
    UsageAuditAggregationQuery, UsageAuditKeywordSearchQuery, UsageAuditListQuery,
    UsageAuditSummaryQuery, UsageBodyCaptureState, UsageBodyField, UsageDashboardSummaryQuery,
    UsageLeaderboardGroupBy, UsageLeaderboardQuery, UsageProviderPerformanceQuery,
    UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
};
use serde_json::json;

#[tokio::test]
async fn customer_billing_statistics_use_frozen_factors_and_preserve_legacy_provider_cost() {
    use aether_data_contracts::repository::usage::*;
    let now = chrono::Utc::now();
    let at = now - chrono::Duration::seconds(10);
    let mut billed = sample_usage("customer-billed", at.timestamp());
    billed.total_cost_usd = 2.0;
    billed.actual_total_cost_usd = 0.5;
    billed.request_metadata = Some(json!({
        "billing_multiplier_snapshot": {
            "version": 1,
            "factors": {"routing_group": 2.0, "user_group": 0.75},
            "multiplier": 1.5
        },
        "routing_group_billing_multiplier": 99.0,
        "rate_multiplier": 0.25
    }));
    let mut legacy = sample_usage("customer-legacy", at.timestamp());
    legacy.total_cost_usd = 2.0;
    legacy.actual_total_cost_usd = 0.5;
    let mut free = sample_usage("customer-free", at.timestamp());
    free.total_cost_usd = 2.0;
    free.actual_total_cost_usd = 0.5;
    free.request_metadata = Some(json!({"routing_group_billing_multiplier": 0.0}));
    let mut invalid = sample_usage("customer-invalid", at.timestamp());
    invalid.total_cost_usd = 999.0;
    invalid.actual_total_cost_usd = 999.0;
    invalid.request_metadata = Some(json!({"billing_multiplier_snapshot": null}));
    let repo = InMemoryUsageReadRepository::seed([billed, legacy, free, invalid])
        .with_dashboard_stats_since(at - chrono::Duration::seconds(1));
    let overview = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            from_unix_ms: (at - chrono::Duration::seconds(1)).timestamp_millis() as u64,
            to_unix_ms: now.timestamp_millis() as u64,
            timezone: "UTC".into(),
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        overview.summary.billable_amount.as_deref(),
        Some("3.50000000")
    );
    let query = UsageDashboardAnalyticsQuery {
        timezone: "UTC".into(),
    };
    let analytics = repo.query_dashboard_analytics(&query).await.unwrap();
    assert_eq!(
        analytics.total.summary.billable_amount.as_deref(),
        Some("3.50000000")
    );
    let summary = repo.query_dashboard_summary(&query).await.unwrap();
    assert_eq!(summary.total.billable_amount.as_deref(), Some("3.50000000"));
    assert_eq!(summary.total.pricing_available_count, 3);
}

#[tokio::test]
async fn overview_model_performance_merges_provider_samples_without_pagination() {
    use aether_data_contracts::repository::usage::*;
    let at = chrono::DateTime::parse_from_rfc3339("2026-09-12T10:05:00Z").unwrap();
    let mut records = Vec::new();
    for (index, provider, first_byte, response_time, output_tokens) in [
        (0, "provider-a", 100, 1100, 100),
        (1, "provider-a", 300, 1300, 200),
        (2, "provider-b", 500, 2500, 400),
    ] {
        let mut row = sample_usage(&format!("model-sample-{index}"), at.timestamp());
        row.provider_id = Some(provider.into());
        row.model = "shared-model".into();
        row.target_model = Some(format!("{provider}-deployment"));
        row.first_byte_time_ms = Some(first_byte);
        row.response_time_ms = Some(response_time);
        row.output_tokens = output_tokens;
        row.is_stream = true;
        records.push(row);
    }
    let mut failed = sample_usage("model-failed", at.timestamp());
    failed.model = "shared-model".into();
    failed.status = "failed".into();
    failed.first_byte_time_ms = None;
    failed.response_time_ms = None;
    records.push(failed);
    let mut pending = sample_usage("model-pending", at.timestamp());
    pending.model = "pending-model".into();
    pending.status = "pending".into();
    pending.first_byte_time_ms = None;
    pending.response_time_ms = None;
    records.push(pending);

    let repo = InMemoryUsageReadRepository::seed(records);
    let query = UsageAnalyticsQuery {
        from_unix_ms: (at - chrono::Duration::minutes(5)).timestamp_millis() as u64,
        to_unix_ms: (at + chrono::Duration::minutes(55)).timestamp_millis() as u64,
        timezone: "UTC".into(),
        view: UsageAnalyticsView::Performance,
        limit: 1,
        offset: 1,
        ..Default::default()
    };
    let result = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(result.model_rows.len(), 2);
    assert_eq!(result.model_rows[0].id.as_deref(), Some("shared-model"));
    assert!(result
        .model_rows
        .iter()
        .all(|row| row.bucket_start.is_none()));
    let metrics = &result.model_rows[0].metrics;
    assert_eq!(metrics.request_count, 4);
    assert_eq!(metrics.successful_request_count, 3);
    assert_eq!(metrics.failed_request_count, 1);
    assert_eq!(metrics.first_byte_sample_count, 3);
    assert_eq!(metrics.first_byte_sum_ms, 900.0);
    assert_eq!(metrics.latency_sample_count, 3);
    assert_eq!(metrics.latency_sum_ms, 4900.0);
    assert_eq!(metrics.output_tps_sample_count, 3);
    assert_eq!(metrics.output_tps_sum, 500.0);
    assert_eq!(result.model_rows[1].metrics.first_byte_sample_count, 0);
    assert_eq!(result.model_rows[1].metrics.in_flight_request_count, 1);
    assert_eq!(
        result
            .model_rows
            .iter()
            .map(|row| row.metrics.request_count)
            .sum::<u64>(),
        result.summary.request_count
    );

    let filtered = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            model: Some("shared-model".into()),
            ..query
        })
        .await
        .unwrap();
    assert_eq!(filtered.model_rows, vec![result.model_rows[0].clone()]);
}

#[tokio::test]
async fn overview_provider_breakdown_labels_rows_with_provider_name() {
    use aether_data_contracts::repository::usage::*;
    let at = chrono::DateTime::parse_from_rfc3339("2026-09-12T10:05:00Z").unwrap();
    // 同一 provider_id 的两条记录，展示标签应解析成提供商名称而不是 provider_id。
    let mut first = sample_usage("provider-label-1", at.timestamp());
    first.provider_id = Some("provider-1".into());
    first.provider_name = "Provider One".into();
    let mut second = sample_usage("provider-label-2", at.timestamp());
    second.provider_id = Some("provider-1".into());
    second.provider_name = "Provider One".into();
    // 名称为历史占位值时回退到 provider_id。
    let mut unnamed = sample_usage("provider-label-3", at.timestamp());
    unnamed.provider_id = Some("provider-2".into());
    unnamed.provider_name = "unknown".into();
    // provider_id 为空说明无法归属，标签保持为空，由前端显示“未归属提供商”。
    let mut unattributed = sample_usage("provider-label-4", at.timestamp());
    unattributed.provider_id = None;
    unattributed.provider_name = "legacy".into();
    let repo = InMemoryUsageReadRepository::seed([first, second, unnamed, unattributed]);
    let result = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            from_unix_ms: (at - chrono::Duration::minutes(5)).timestamp_millis() as u64,
            to_unix_ms: (at + chrono::Duration::minutes(55)).timestamp_millis() as u64,
            timezone: "UTC".into(),
            view: UsageAnalyticsView::Breakdown,
            group_by: UsageAnalyticsGroupBy::Provider,
            limit: 25,
            descending: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.total, 3);
    let rows = result
        .rows
        .iter()
        .map(|row| (row.id.as_deref(), row.label.as_deref()))
        .collect::<Vec<_>>();
    assert!(rows.contains(&(Some("provider-1"), Some("Provider One"))));
    assert!(rows.contains(&(Some("provider-2"), Some("provider-2"))));
    assert!(rows.contains(&(None, None)));
    // 明细分组不填充 bucket_start。
    assert!(result.rows.iter().all(|row| row.bucket_start.is_none()));
}

#[tokio::test]
async fn overview_memory_chart_hour_buckets_are_utc_in_half_hour_zones() {
    use aether_data_contracts::repository::usage::*;
    let at = chrono::DateTime::parse_from_rfc3339("2026-09-12T10:05:00Z").unwrap();
    let repo = InMemoryUsageReadRepository::seed([sample_usage("hour-zone", at.timestamp())]);
    let query = UsageAnalyticsQuery {
        from_unix_ms: (at - chrono::Duration::minutes(5)).timestamp_millis() as u64,
        to_unix_ms: (at + chrono::Duration::minutes(55)).timestamp_millis() as u64,
        timezone: "Asia/Kolkata".into(),
        view: UsageAnalyticsView::DashboardCharts,
        granularity: UsageAnalyticsGranularity::Hour,
        limit: 1,
        ..Default::default()
    };
    let charts = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(charts.summary.request_count, 1);
    assert_eq!(charts.rows.len(), 1);
    assert_eq!(charts.rows[0].metrics.request_count, 1);
    assert_eq!(
        charts.rows[0].bucket_start,
        charts.model_rows[0].bucket_start
    );
}

#[tokio::test]
async fn overview_memory_chart_days_survive_skipped_midnight() {
    use aether_data_contracts::repository::usage::*;
    let moments = [
        "2026-09-05T12:00:00Z",
        "2026-09-06T12:00:00Z",
        "2026-09-07T12:00:00Z",
    ];
    let repo = InMemoryUsageReadRepository::seed(moments.map(|moment| {
        sample_usage(
            moment,
            chrono::DateTime::parse_from_rfc3339(moment)
                .unwrap()
                .timestamp(),
        )
    }));
    let query = UsageAnalyticsQuery {
        from_unix_ms: chrono::DateTime::parse_from_rfc3339("2026-09-05T04:00:00Z")
            .unwrap()
            .timestamp_millis() as u64,
        to_unix_ms: chrono::DateTime::parse_from_rfc3339("2026-09-08T03:00:00Z")
            .unwrap()
            .timestamp_millis() as u64,
        timezone: "America/Santiago".into(),
        view: UsageAnalyticsView::DashboardCharts,
        limit: 1,
        ..Default::default()
    };
    let charts = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(charts.summary.request_count, 3);
    assert_eq!(charts.rows.len(), 3);
    assert_eq!(charts.model_rows.len(), 3);
    for (series, model) in charts.rows.iter().zip(&charts.model_rows) {
        assert_eq!(series.metrics.request_count, 1);
        assert_eq!(series.bucket_start, model.bucket_start);
        assert_eq!(
            series.metrics.billable_amount,
            model.metrics.billable_amount
        );
    }
}

#[tokio::test]
async fn overview_memory_dashboard_keeps_all_history_and_chart_dimensions() {
    use aether_data_contracts::repository::usage::*;
    let now = chrono::Utc::now();
    let mut old = sample_usage(
        "old-dashboard",
        (now - chrono::Duration::days(800)).timestamp(),
    );
    old.actual_total_cost_usd = 2.0;
    old.model = "old-model".into();
    let mut current = sample_usage("current-dashboard", now.timestamp());
    current.actual_total_cost_usd = 0.5;
    current.model = "new-model".into();
    let repo = InMemoryUsageReadRepository::seed([old, current]);
    let dashboard = repo
        .query_dashboard_analytics(&UsageDashboardAnalyticsQuery {
            timezone: "Asia/Shanghai".into(),
        })
        .await
        .unwrap();
    assert_eq!(dashboard.today.summary.request_count, 1);
    assert_eq!(dashboard.total.summary.request_count, 2);
    assert_eq!(
        dashboard.today.summary.billable_amount.as_deref(),
        Some("0.50000000")
    );
    assert_eq!(
        dashboard.total.summary.billable_amount.as_deref(),
        Some("2.50000000")
    );
    assert_eq!(dashboard.today.read_revision, dashboard.total.read_revision);
    assert_eq!(dashboard.history_complete, None);
    let charts = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            from_unix_ms: (now - chrono::Duration::days(1)).timestamp_millis() as u64,
            to_unix_ms: (now + chrono::Duration::seconds(1)).timestamp_millis() as u64,
            timezone: "Asia/Shanghai".into(),
            view: UsageAnalyticsView::DashboardCharts,
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(charts.model_rows.len(), 1);
    assert_eq!(charts.model_rows[0].id.as_deref(), Some("new-model"));
    assert_eq!(charts.provider_rows.len(), 1);
    assert_eq!(
        charts.model_rows[0].metrics.billable_amount,
        charts.summary.billable_amount
    );
}

#[tokio::test]
async fn overview_memory_dashboard_total_keeps_card_coverage_without_historical_diagnostics() {
    use aether_data_contracts::repository::usage::*;
    let now = chrono::Utc::now();
    let mut known = sample_usage("dashboard-total-known", now.timestamp());
    known.request_metadata = Some(json!({"analytics_attribution":{"is_standalone":false}}));
    let mut missing = sample_usage(
        "dashboard-total-missing",
        (now - chrono::Duration::days(800)).timestamp(),
    );
    missing.billing_status = "pending".into();
    missing.request_metadata =
        Some(json!({"usage_available":false,"usage_pricing_available":false}));
    let mut session = sample_usage("dashboard-total-session", now.timestamp());
    session.request_metadata = Some(json!({"analytics_attribution":{"record_kind":"session"}}));
    let future = sample_usage(
        "dashboard-total-future",
        (now + chrono::Duration::days(1)).timestamp(),
    );
    let repo = InMemoryUsageReadRepository::seed([known, missing, session, future])
        .with_analytics_allocations([UsageAnalyticsAllocation {
            request_id: "dashboard-total-known".into(),
            complete: true,
            wallet_debit_amount: Some("0.18000000".into()),
            ..Default::default()
        }]);
    let dashboard = repo
        .query_dashboard_analytics(&UsageDashboardAnalyticsQuery {
            timezone: "UTC".into(),
        })
        .await
        .unwrap();
    let total = dashboard.total.summary;
    assert_eq!(total.request_count, 2);
    assert_eq!(total.total_tokens, 150);
    assert_eq!(total.billable_amount.as_deref(), Some("0.18000000"));
    assert_eq!(total.usage_available_count, 1);
    assert_eq!(total.pricing_available_count, 1);
    assert_eq!(total.settled_count, 1);
    assert_eq!(total.allocation_available_count, 1);
    assert!(total.latency_p95_ms.is_none());
    assert!(total.wallet_debit_amount.is_none());
    assert_eq!(dashboard.today.summary.latency_p95_ms, Some(420.0));
    assert_eq!(dashboard.today.summary.successful_request_count, 1);
    assert_eq!(
        dashboard.today.summary.wallet_debit_amount.as_deref(),
        Some("0.18000000")
    );
}

#[tokio::test]
async fn overview_memory_employee_roster_and_allocations_are_global() {
    use aether_data_contracts::repository::usage::*;
    use aether_data_contracts::repository::users::StoredUserSummary;
    let mut usage = sample_usage("overview-request", 1_700_000_000);
    usage.user_id = Some("alice".into());
    usage.request_metadata = Some(json!({"analytics_attribution":{"is_standalone":false}}));
    usage.billing_status = "settled".into();
    usage.actual_total_cost_usd = 0.00000001;
    let repo = InMemoryUsageReadRepository::seed([usage])
        .with_analytics_users([
            StoredUserSummary::new(
                "alice".into(),
                "Alice".into(),
                None,
                "user".into(),
                true,
                false,
            )
            .unwrap(),
            StoredUserSummary::new(
                "zero".into(),
                "Zero".into(),
                None,
                "user".into(),
                true,
                false,
            )
            .unwrap(),
        ])
        .with_analytics_allocations([UsageAnalyticsAllocation {
            request_id: "overview-request".into(),
            quota_covered_amount: Some("0.00000000".into()),
            wallet_consumed_amount: Some("0.00000001".into()),
            wallet_debit_amount: Some("0.00000000".into()),
            complete: true,
            ..Default::default()
        }]);
    let mut query = UsageAnalyticsQuery {
        from_unix_ms: 1_700_000_000_000,
        to_unix_ms: 1_700_000_060_000,
        timezone: "UTC".into(),
        view: UsageAnalyticsView::Users,
        limit: 1,
        descending: true,
        ..Default::default()
    };
    let first = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(first.users[0].user_id, "alice");
    assert_eq!(first.user_summary.as_ref().unwrap().user_count, 2);
    assert_eq!(first.user_summary.as_ref().unwrap().active_user_count, 1);
    assert!(first.user_finance_summary.is_none());
    assert!(first.user_payments.is_none());
    assert!(first.users[0].finance.is_none());
    assert_eq!(
        first.summary.wallet_consumed_amount.as_deref(),
        Some("0.00000001")
    );
    assert_eq!(
        first.summary.wallet_debit_amount.as_deref(),
        Some("0.00000000")
    );
    query.offset = 1;
    let second = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(second.users[0].user_id, "zero");
    assert_eq!(second.users[0].metrics.request_count, 0);
    assert_eq!(second.user_summary, first.user_summary);
    let searched = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            search: Some("Zero".into()),
            offset: 0,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(searched.user_summary.as_ref().unwrap().user_count, 1);
    assert_eq!(searched.user_summary.as_ref().unwrap().active_user_count, 0);
    assert_eq!(searched.summary.request_count, 0);
    query.from_unix_ms = query.to_unix_ms;
    query.to_unix_ms += 60_000;
    let outside = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(outside.summary.request_count, 0);
}

#[tokio::test]
async fn overview_memory_employee_is_grouped_by_account_owner() {
    use aether_data_contracts::repository::{usage::*, users::StoredUserSummary};
    let mut usage = sample_usage("trusted-request", 1_700_000_000);
    usage.user_id = Some("owner".into());
    usage.request_metadata =
        Some(json!({"analytics_attribution":{"is_standalone":false,"actor_user_id":"actor"}}));
    let repo = InMemoryUsageReadRepository::seed([usage]).with_analytics_users(
        ["owner", "actor"].map(|id| {
            StoredUserSummary::new(id.into(), id.into(), None, "user".into(), true, false).unwrap()
        }),
    );
    let query = UsageAnalyticsQuery {
        from_unix_ms: 1_700_000_000_000,
        to_unix_ms: 1_700_000_060_000,
        timezone: "UTC".into(),
        view: UsageAnalyticsView::Users,
        attribution_kind: Some("employee".into()),
        has_usage: Some(true),
        limit: 100,
        ..Default::default()
    };
    let result = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.users[0].user_id, "owner");
    let detail = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            actor_user_id: Some("owner".into()),
            ..query
        })
        .await
        .unwrap();
    assert_eq!(detail.users[0], result.users[0]);
}

#[tokio::test]
async fn overview_memory_legacy_requests_use_existing_key_account_flags() {
    use aether_data_contracts::repository::{usage::*, users::StoredUserSummary};
    let snapshots = [("member-key", false), ("standalone-key", true)].map(|(id, standalone)| {
        (
            None,
            StoredAuthApiKeySnapshot::new(
                "user-1".into(),
                "alice".into(),
                None,
                "user".into(),
                "local".into(),
                true,
                false,
                None,
                None,
                None,
                id.into(),
                None,
                true,
                false,
                standalone,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap(),
        )
    });
    let keys = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(snapshots));
    let rows = ["member-key", "standalone-key", "deleted-key"].map(|id| {
        let mut usage = sample_usage(id, 1_700_000_000);
        usage.api_key_id = Some(id.into());
        usage.request_metadata = None;
        usage
    });
    let repo = InMemoryUsageReadRepository::seed(rows)
        .with_auth_api_key_repository(keys)
        .with_analytics_users([StoredUserSummary::new(
            "user-1".into(),
            "alice".into(),
            None,
            "user".into(),
            true,
            false,
        )
        .unwrap()]);
    let mut query = UsageAnalyticsQuery {
        from_unix_ms: 1_700_000_000_000,
        to_unix_ms: 1_700_000_060_000,
        timezone: "UTC".into(),
        view: UsageAnalyticsView::Consumption,
        limit: 100,
        ..Default::default()
    };
    let result = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(result.summary.request_count, 3);
    assert_eq!(result.summary.usage_active_users, 1);
    assert_eq!(result.summary.trusted_attribution_count, 1);
    for (key, kind, source, user) in [
        ("member-key", "employee", "user_account", Some("user-1")),
        ("standalone-key", "standalone", "standalone_key", None),
        ("deleted-key", "unknown", "unknown", None),
    ] {
        let row = result
            .consumption
            .iter()
            .find(|row| row.request_id == key)
            .unwrap();
        assert_eq!(row.attribution_kind, kind);
        assert_eq!(row.attribution_source, source);
        assert_eq!(row.user_id.as_deref(), user);
        assert_eq!(row.credential_owner_id.as_deref(), Some("user-1"));
    }
    query.view = UsageAnalyticsView::Users;
    query.attribution_kind = Some("employee".into());
    let employees = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(employees.users[0].metrics.request_count, 1);
    assert_eq!(employees.summary.request_count, 1);
}

#[tokio::test]
async fn overview_memory_health_does_not_treat_upstream_cancellation_as_client() {
    use aether_data_contracts::repository::usage::*;
    let mut upstream = sample_usage("upstream-cancel", 1_700_000_000);
    upstream.status = "cancelled".into();
    upstream.request_metadata =
        Some(json!({"analytics_failure":{"origin":"upstream","reason":"provider_cancelled"}}));
    let mut client = upstream.clone();
    client.request_id = "client-cancel".into();
    client.request_metadata =
        Some(json!({"analytics_failure":{"origin":"client","reason":"downstream_disconnect"}}));
    let repo = InMemoryUsageReadRepository::seed([upstream, client]);
    let summary = repo
        .summarize_health_observations(&HealthObservationQuery {
            from_unix_ms: 1_700_000_000_000,
            to_unix_ms: 1_700_000_060_000,
            object_kind: HealthObservationObjectKind::Model,
            object_values: None,
            segments: 4,
        })
        .await
        .unwrap();
    assert_eq!(summary.overall.request_count, 2);
    assert_eq!(summary.overall.service_failed_count, 1);
    assert_eq!(summary.overall.excluded_count, 1);
}

fn sample_usage(request_id: &str, created_at_unix_ms: i64) -> StoredRequestUsageAudit {
    StoredRequestUsageAudit::new(
        "usage-1".to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        "OpenAI".to_string(),
        "gpt-4.1".to_string(),
        Some("gpt-4.1-mini".to_string()),
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
        false,
        100,
        50,
        150,
        0.12,
        0.18,
        Some(200),
        None,
        None,
        Some(420),
        Some(120),
        "completed".to_string(),
        "settled".to_string(),
        created_at_unix_ms,
        created_at_unix_ms + 1,
        Some(created_at_unix_ms + 2),
    )
    .expect("usage should build")
}

fn sample_upsert_usage_record(request_id: &str) -> UpsertUsageRecord {
    UpsertUsageRecord {
        capture_retention: Default::default(),
        request_id: request_id.to_string(),
        user_id: None,
        api_key_id: None,
        username: None,
        api_key_name: None,
        provider_name: "OpenAI".to_string(),
        model: "gpt-5".to_string(),
        target_model: None,
        provider_id: Some("provider-1".to_string()),
        provider_endpoint_id: None,
        provider_api_key_id: None,
        request_type: None,
        api_format: None,
        api_family: None,
        endpoint_kind: None,
        endpoint_api_format: None,
        provider_api_family: None,
        provider_endpoint_kind: None,
        has_format_conversion: Some(false),
        is_stream: Some(false),
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        cache_creation_input_tokens: None,
        cache_creation_ephemeral_5m_input_tokens: None,
        cache_creation_ephemeral_1h_input_tokens: None,
        cache_read_input_tokens: None,
        cache_creation_cost_usd: None,
        cache_read_cost_usd: None,
        output_price_per_1m: None,
        total_cost_usd: None,
        actual_total_cost_usd: None,
        status_code: None,
        error_message: None,
        error_category: None,
        response_time_ms: None,
        first_byte_time_ms: None,
        status: "pending".to_string(),
        billing_status: "pending".to_string(),
        request_headers: None,
        request_body: None,
        request_body_ref: None,
        request_body_state: None,
        provider_request_headers: None,
        provider_request_body: None,
        provider_request_body_ref: None,
        provider_request_body_state: None,
        response_headers: None,
        response_body: None,
        response_body_ref: None,
        response_body_state: None,
        client_response_headers: None,
        client_response_body: None,
        client_response_body_ref: None,
        client_response_body_state: None,
        candidate_id: None,
        candidate_index: None,
        key_name: None,
        planner_kind: None,
        route_family: None,
        route_kind: None,
        execution_path: None,
        local_execution_runtime_miss_reason: None,
        request_metadata: None,
        finalized_at_unix_secs: None,
        created_at_unix_ms: Some(1_700_000_000),
        updated_at_unix_secs: 1_700_000_000,
    }
}

#[tokio::test]
async fn upsert_preserves_routing_group_snapshot_across_terminal_metadata_replacement() {
    for terminal_metadata in [
        None,
        Some(json!({"rate_multiplier": 0.5, "billing_snapshot": {"status": "complete"}})),
        Some(json!({
            "routing_group_billing_multiplier": 99.0,
            "billing_multiplier_snapshot": {"version": 1, "factors": {"routing_group": 3.0}, "multiplier": 3.0},
            "routing_group_id": "changed-group",
            "routing_group_name": "changed-group-name",
            "plan_usage_reservation_token": "550e8400-e29b-41d4-a716-446655440002",
            "rate_multiplier": 0.5
        })),
    ] {
        let repository = InMemoryUsageReadRepository::default();
        let mut pending = sample_upsert_usage_record("req-group-snapshot");
        pending.request_metadata = Some(json!({
            "routing_group_billing_multiplier": 0.25,
            "billing_multiplier_snapshot": {"version": 1, "factors": {"routing_group": 0.25, "user_group": 2.0}, "multiplier": 0.5},
            "routing_group_id": "group-original",
            "routing_group_name": "请求时的分组",
            "plan_usage_reservation_token": "550e8400-e29b-41d4-a716-446655440001"
        }));
        repository
            .upsert(pending)
            .await
            .expect("pending usage should persist");
        let mut terminal = sample_upsert_usage_record("req-group-snapshot");
        terminal.status = "completed".to_string();
        terminal.request_metadata = terminal_metadata;
        terminal.updated_at_unix_secs += 1;
        let stored = repository
            .upsert(terminal)
            .await
            .expect("terminal usage should persist");
        assert_eq!(stored.routing_group_billing_multiplier(), 0.25);
        assert_eq!(stored.billing_multiplier(), 0.5);
        assert_eq!(
            stored.request_metadata.as_ref().unwrap()["billing_multiplier_snapshot"],
            json!({
                "version": 1, "factors": {"routing_group": 0.25, "user_group": 2.0}, "multiplier": 0.5
            })
        );
        assert_eq!(stored.routing_group_id(), Some("group-original"));
        assert_eq!(stored.routing_group_name(), Some("请求时的分组"));
        assert_eq!(
            stored.request_metadata.as_ref().unwrap()["plan_usage_reservation_token"],
            "550e8400-e29b-41d4-a716-446655440001"
        );
    }
}

#[tokio::test]
async fn upsert_preserves_full_http_captures_across_lifecycle_updates() {
    let repository = InMemoryUsageReadRepository::default();
    let mut pending = sample_upsert_usage_record("req-full-capture");
    pending.request_headers =
        Some(json!({"content-type": "application/json", "authorization": "Bearer private"}));
    pending.request_body =
        Some(json!({"messages": [{"role": "user", "content": "original request"}]}));
    pending.provider_request_body = Some(json!({"input": "provider request"}));
    pending.request_body_state = Some(UsageBodyCaptureState::Inline);
    pending.provider_request_body_state = Some(UsageBodyCaptureState::Inline);
    let stored_pending = repository.upsert(pending.clone()).await.unwrap();
    assert_eq!(stored_pending.request_body, pending.request_body);
    assert_eq!(
        stored_pending.provider_request_body,
        pending.provider_request_body
    );
    assert_eq!(
        stored_pending.request_headers,
        Some(json!({"content-type": "application/json", "authorization": "Bearer private"}))
    );

    let mut streaming = sample_upsert_usage_record(&pending.request_id);
    streaming.status = "streaming".to_string();
    streaming.updated_at_unix_secs += 1;
    let stored_streaming = repository.upsert(streaming).await.unwrap();
    assert_eq!(stored_streaming.request_body, pending.request_body);
    assert_eq!(
        stored_streaming.provider_request_body,
        pending.provider_request_body
    );

    let mut terminal = sample_upsert_usage_record(&pending.request_id);
    terminal.status = "completed".to_string();
    terminal.updated_at_unix_secs += 2;
    terminal.finalized_at_unix_secs = Some(terminal.updated_at_unix_secs);
    terminal.response_headers =
        Some(json!({"content-type": "text/event-stream", "set-cookie": "private"}));
    terminal.response_body = Some(json!("data: upstream response\n\ndata: [DONE]\n\n"));
    terminal.client_response_body =
        Some(json!({"choices": [{"message": {"content": "client response"}}]}));
    let stored_terminal = repository.upsert(terminal.clone()).await.unwrap();
    assert_eq!(stored_terminal.request_body, pending.request_body);
    assert_eq!(
        stored_terminal.provider_request_body,
        pending.provider_request_body
    );
    assert_eq!(stored_terminal.response_body, terminal.response_body);
    assert_eq!(
        stored_terminal.client_response_body,
        terminal.client_response_body
    );
    assert_eq!(
        stored_terminal.response_headers,
        Some(json!({"content-type": "text/event-stream", "set-cookie": "private"}))
    );

    let found = repository
        .find_by_request_id(&pending.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.request_body, pending.request_body);
    assert_eq!(found.response_body, terminal.response_body);
    assert_eq!(
        repository.upsert(pending).await.unwrap().response_body,
        terminal.response_body
    );
}

#[tokio::test]
async fn upsert_uses_typed_provider_capture_as_the_fast_fact_snapshot() {
    for (name, state, incoming_tier, expected_tier) in [
        (
            "disabled-clear",
            UsageBodyCaptureState::Disabled,
            None,
            None,
        ),
        (
            "truncated-clear",
            UsageBodyCaptureState::Truncated,
            None,
            None,
        ),
        (
            "unavailable-clear",
            UsageBodyCaptureState::Unavailable,
            None,
            None,
        ),
        (
            "disabled-preserve",
            UsageBodyCaptureState::Disabled,
            Some("priority"),
            Some("priority"),
        ),
        (
            "truncated-preserve",
            UsageBodyCaptureState::Truncated,
            Some("priority"),
            Some("priority"),
        ),
        (
            "none-clears-residual",
            UsageBodyCaptureState::None,
            Some("priority"),
            None,
        ),
    ] {
        let request_id = format!("req-memory-fast-{name}");
        let repository = InMemoryUsageReadRepository::default();
        let mut pending = sample_upsert_usage_record(&request_id);
        pending.provider_request_body = Some(json!({
            "model": "gpt-5",
            "service_tier": "priority"
        }));
        pending.provider_request_body_state = Some(UsageBodyCaptureState::Inline);
        pending.request_metadata = Some(json!({"provider_service_tier": "priority"}));
        pending.target_model = Some("candidate-a-target".to_string());
        pending.candidate_id = Some("candidate-a".to_string());
        pending.candidate_index = Some(1);
        pending.key_name = Some("key-a".to_string());
        pending.planner_kind = Some("planner-a".to_string());
        pending.route_family = Some("route-family-a".to_string());
        pending.route_kind = Some("route-kind-a".to_string());
        pending.execution_path = Some("path-a".to_string());
        let pending = repository
            .upsert(pending)
            .await
            .expect("pending usage should upsert");
        assert_eq!(pending.provider_service_tier().as_deref(), Some("priority"));

        let mut terminal = sample_upsert_usage_record(&request_id);
        terminal.status = "completed".to_string();
        terminal.provider_request_body_state = Some(state);
        terminal.target_model = None;
        terminal.request_metadata =
            incoming_tier.map(|tier| json!({"provider_service_tier": tier}));
        terminal.updated_at_unix_secs += 1;
        terminal.finalized_at_unix_secs = Some(terminal.updated_at_unix_secs);
        if state == UsageBodyCaptureState::None {
            terminal.provider_request_body = Some(json!({
                "model": "stale-model",
                "service_tier": "priority"
            }));
            terminal.provider_request_body_ref = Some(usage_body_ref(
                &request_id,
                UsageBodyField::ProviderRequestBody,
            ));
        }
        let terminal = repository
            .upsert(terminal)
            .await
            .expect("terminal usage should upsert");
        assert_eq!(
            terminal.provider_service_tier().as_deref(),
            expected_tier,
            "state={state:?} must use only incoming facts"
        );
        assert_eq!(terminal.target_model, None);
        assert_eq!(terminal.candidate_id, None);
        assert_eq!(terminal.candidate_index, None);
        assert_eq!(terminal.key_name, None);
        assert_eq!(terminal.planner_kind, None);
        assert_eq!(terminal.route_family, None);
        assert_eq!(terminal.route_kind, None);
        assert_eq!(terminal.execution_path, None);
        if state == UsageBodyCaptureState::None {
            assert_eq!(terminal.provider_request_body, None);
            assert_eq!(terminal.provider_request_body_ref, None);

            let mut late = sample_upsert_usage_record(&request_id);
            late.provider_request_body = Some(json!({
                "model": "late-model",
                "service_tier": "priority"
            }));
            late.provider_request_body_state = Some(UsageBodyCaptureState::Inline);
            late.request_metadata = Some(json!({"provider_service_tier": "priority"}));
            late.updated_at_unix_secs += 2;
            let late = repository
                .upsert(late)
                .await
                .expect("late pending usage should not regress terminal capture");
            assert_eq!(late.provider_service_tier(), None);
            assert_eq!(late.provider_request_body, None);
            assert_eq!(late.provider_request_body_ref, None);
        }
    }
}

#[tokio::test]
async fn finds_usage_by_request_id() {
    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-1", 100),
        sample_usage("req-2", 200),
    ]);

    let usage = repository
        .find_by_request_id("req-2")
        .await
        .expect("find should succeed")
        .expect("usage should exist");

    assert_eq!(usage.request_id, "req-2");
    assert_eq!(usage.total_tokens, 150);
}

#[tokio::test]
async fn provider_aggregation_skips_unknown_provider_labels() {
    let valid_provider = sample_usage("req-valid-provider", 300);

    let mut legacy_provider = sample_usage("req-legacy-provider", 250);
    legacy_provider.provider_id = None;
    legacy_provider.provider_name = "Legacy Provider".to_string();

    let mut unknown = sample_usage("req-unknown-provider", 100);
    unknown.provider_id = None;
    unknown.provider_name = "unknown".to_string();

    let mut typo_unknown = sample_usage("req-unknow-provider", 200);
    typo_unknown.provider_id = Some("unknow".to_string());
    typo_unknown.provider_name = "unknow".to_string();

    let repository = InMemoryUsageReadRepository::seed(vec![
        valid_provider,
        legacy_provider,
        unknown,
        typo_unknown,
    ]);

    let rows = repository
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 1_000,
            group_by: UsageAuditAggregationGroupBy::Provider,
            limit: 10,
            exclude_reserved_provider_labels: false,
        })
        .await
        .expect("aggregation should succeed");

    assert_eq!(rows.len(), 2);
    let provider_id_row = rows
        .iter()
        .find(|row| row.group_key == "provider-1")
        .expect("provider_id row should be present");
    assert_eq!(provider_id_row.display_name.as_deref(), Some("OpenAI"));
    assert_eq!(
        provider_id_row.secondary_name.as_deref(),
        Some("provider_id")
    );

    let legacy_name_row = rows
        .iter()
        .find(|row| row.group_key == "Legacy Provider")
        .expect("legacy provider name row should be present");
    assert_eq!(
        legacy_name_row.display_name.as_deref(),
        Some("Legacy Provider")
    );
    assert_eq!(
        legacy_name_row.secondary_name.as_deref(),
        Some("legacy_name")
    );
}

#[tokio::test]
async fn unmetered_session_audit_counts_lifecycle_without_token_or_cost_contribution() {
    let metered = sample_usage("req-metered", 100);
    let mut live = sample_usage("req-live", 200);
    live.request_metadata = Some(json!({
        "usage_available": false,
        "websocket_mode": true,
        "websocket_transport": "codex_live_direct",
    }));
    live.billing_status = "void".to_string();
    live.input_tokens = 0;
    live.output_tokens = 0;
    live.total_tokens = 0;
    live.cache_creation_input_tokens = 0;
    live.cache_creation_ephemeral_5m_input_tokens = 0;
    live.cache_creation_ephemeral_1h_input_tokens = 0;
    live.cache_read_input_tokens = 0;
    live.total_cost_usd = 0.0;
    live.actual_total_cost_usd = 0.0;
    let repository = InMemoryUsageReadRepository::seed(vec![metered, live]);

    let listed = repository
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(0),
            created_until_unix_secs: Some(1_000),
            newest_first: true,
            ..UsageAuditListQuery::default()
        })
        .await
        .expect("audit list should succeed");
    assert_eq!(listed.len(), 2);
    assert!(!listed
        .iter()
        .find(|item| item.request_id == "req-live")
        .expect("Live row should remain visible")
        .usage_available());

    let aggregate = repository
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 1_000,
            group_by: UsageAuditAggregationGroupBy::Model,
            limit: 10,
            exclude_reserved_provider_labels: false,
        })
        .await
        .expect("aggregate should succeed");
    assert_eq!(aggregate.len(), 1);
    assert_eq!(aggregate[0].request_count, 2);
    assert_eq!(aggregate[0].total_tokens, 150);

    let summary = repository
        .summarize_usage_audits(&UsageAuditSummaryQuery {
            provider_names: None,
            created_from_unix_secs: 0,
            created_until_unix_secs: 1_000,
            ..UsageAuditSummaryQuery::default()
        })
        .await
        .expect("summary should succeed");
    assert_eq!(summary.total_requests, 2);
    assert_eq!(summary.recorded_total_tokens, 150);

    let provider_key_summaries = repository
        .summarize_usage_by_provider_api_key_ids(&["provider-key-1".to_string()])
        .await
        .expect("provider key lifecycle summary should succeed");
    let provider_key_summary = provider_key_summaries
        .get("provider-key-1")
        .expect("provider key summary");
    assert_eq!(provider_key_summary.request_count, 2);
    assert_eq!(provider_key_summary.total_tokens, 150);
}

#[tokio::test]
async fn aggregation_can_skip_unknown_provider_records_for_model_and_api_format() {
    let mut unknown = sample_usage("req-unknown-provider", 100);
    unknown.provider_id = None;
    unknown.provider_name = "unknown".to_string();

    let mut typo_unknown = sample_usage("req-unknow-provider", 200);
    typo_unknown.provider_id = Some("unknow".to_string());
    typo_unknown.provider_name = "unknow".to_string();

    let mut pending_provider = sample_usage("req-pending-provider", 300);
    pending_provider.provider_id = None;
    pending_provider.provider_name = "pending".to_string();

    let mut id_only_provider = sample_usage("req-id-only-provider", 350);
    id_only_provider.provider_name = "unknown".to_string();

    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-valid-provider", 400),
        unknown,
        typo_unknown,
        pending_provider,
        id_only_provider,
    ]);

    let model_rows = repository
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 1_000,
            group_by: UsageAuditAggregationGroupBy::Model,
            limit: 10,
            exclude_reserved_provider_labels: true,
        })
        .await
        .expect("model aggregation should succeed");
    assert_eq!(model_rows.len(), 1);
    assert_eq!(model_rows[0].group_key, "gpt-4.1");
    assert_eq!(model_rows[0].request_count, 2);

    let api_format_rows = repository
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 1_000,
            group_by: UsageAuditAggregationGroupBy::ApiFormat,
            limit: 10,
            exclude_reserved_provider_labels: true,
        })
        .await
        .expect("api format aggregation should succeed");
    assert_eq!(api_format_rows.len(), 1);
    assert_eq!(api_format_rows[0].group_key, "openai:chat");
    assert_eq!(api_format_rows[0].request_count, 2);
}

#[tokio::test]
async fn stale_pending_update_does_not_regress_finalized_usage() {
    let repository = InMemoryUsageReadRepository::default();
    repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-finalized-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(3),
            output_tokens: Some(5),
            total_tokens: Some(8),
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: None,
            actual_total_cost_usd: None,
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(45),
            first_byte_time_ms: None,
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: Some(101),
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("completed usage should upsert");

    repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-finalized-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: None,
            actual_total_cost_usd: None,
            status_code: None,
            error_message: None,
            error_category: None,
            response_time_ms: None,
            first_byte_time_ms: None,
            status: "pending".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 102,
        })
        .await
        .expect("stale pending usage should upsert");

    let stored = repository
        .find_by_request_id("req-finalized-1")
        .await
        .expect("usage lookup should succeed")
        .expect("usage should exist");
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.status_code, Some(200));
    assert_eq!(stored.total_tokens, 8);
    assert_eq!(stored.finalized_at_unix_secs, Some(101));
}

#[tokio::test]
async fn upsert_allows_completed_recovery_after_void_failure() {
    let repository = InMemoryUsageReadRepository::default();
    repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-recover-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.0),
            actual_total_cost_usd: Some(0.0),
            status_code: Some(503),
            error_message: Some("provider timeout".to_string()),
            error_category: Some("provider_error".to_string()),
            response_time_ms: Some(90),
            first_byte_time_ms: None,
            status: "failed".to_string(),
            billing_status: "void".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: Some(101),
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("failed usage should upsert");

    repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-recover-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: Some("gpt-5-mini".to_string()),
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(true),
            is_stream: Some(true),
            input_tokens: Some(10),
            output_tokens: None,
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: None,
            actual_total_cost_usd: None,
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(45),
            first_byte_time_ms: Some(12),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: Some("cand-1".to_string()),
            candidate_index: Some(1),
            key_name: Some("primary".to_string()),
            planner_kind: Some("claude_cli_sync".to_string()),
            route_family: Some("claude".to_string()),
            route_kind: Some("cli".to_string()),
            execution_path: Some("remote".to_string()),
            local_execution_runtime_miss_reason: None,
            request_metadata: Some(json!({
                "trace_id": "trace-recovered"
            })),
            finalized_at_unix_secs: Some(102),
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 102,
        })
        .await
        .expect("recovery usage should upsert");

    let stored = repository
        .find_by_request_id("req-recover-1")
        .await
        .expect("usage lookup should succeed")
        .expect("usage should exist");
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.billing_status, "pending");
    assert_eq!(stored.status_code, Some(200));
    assert_eq!(stored.error_message, None);
    assert_eq!(stored.finalized_at_unix_secs, Some(102));
    assert_eq!(
        stored.request_metadata,
        Some(json!({ "trace_id": "trace-recovered" }))
    );
    assert_eq!(stored.total_tokens, 10);
}

#[tokio::test]
async fn stale_terminal_event_cannot_replace_usage_routing_or_counter_contribution() {
    let auth_api_keys = sample_auth_api_key_repository(&["api-key-1"]);
    let repository = InMemoryUsageReadRepository::default()
        .with_auth_api_key_repository(Arc::clone(&auth_api_keys));

    let mut newer = sample_upsert_usage_record("req-stale-terminal");
    newer.api_key_id = Some("api-key-1".to_string());
    newer.status = "completed".to_string();
    newer.status_code = Some(200);
    newer.total_tokens = Some(5);
    newer.total_cost_usd = Some(0.5);
    newer.candidate_id = Some("candidate-new".to_string());
    newer.route_kind = Some("route-new".to_string());
    newer.updated_at_unix_secs = 200;
    newer.finalized_at_unix_secs = Some(200);
    repository
        .upsert(newer)
        .await
        .expect("newer terminal usage should upsert");

    let mut stale = sample_upsert_usage_record("req-stale-terminal");
    stale.api_key_id = Some("api-key-1".to_string());
    stale.status = "failed".to_string();
    stale.billing_status = "void".to_string();
    stale.status_code = Some(503);
    stale.total_tokens = Some(999);
    stale.total_cost_usd = Some(99.0);
    stale.candidate_id = Some("candidate-stale".to_string());
    stale.route_kind = Some("route-stale".to_string());
    stale.updated_at_unix_secs = 199;
    stale.finalized_at_unix_secs = Some(199);
    let stored = repository
        .upsert(stale)
        .await
        .expect("stale terminal usage should be ignored");

    assert_eq!(stored.status, "completed");
    assert_eq!(stored.billing_status, "pending");
    assert_eq!(stored.status_code, Some(200));
    assert_eq!(stored.total_tokens, 5);
    assert_eq!(stored.total_cost_usd, 0.5);
    assert_eq!(stored.routing_candidate_id(), Some("candidate-new"));
    assert_eq!(stored.routing_route_kind(), Some("route-new"));
    assert_eq!(stored.updated_at_unix_secs, 200);

    let key = auth_api_keys
        .list_export_api_keys_by_ids(&["api-key-1".to_string()])
        .await
        .expect("api key stats should load")
        .into_iter()
        .next()
        .expect("api key should exist");
    assert_eq!(key.total_requests, 1);
    assert_eq!(key.total_tokens, 5);
    assert_eq!(key.total_cost_usd, 0.5);
}

#[tokio::test]
async fn upsert_rejects_non_authoritative_void_failure_recovery() {
    let repository = InMemoryUsageReadRepository::default();
    for (request_id, status, billing_status, status_code) in [
        ("req-late-active-1", "streaming", "pending", None),
        (
            "req-late-response-start-1",
            "streaming",
            "pending",
            Some(200),
        ),
        (
            "req-settled-completion-1",
            "completed",
            "settled",
            Some(200),
        ),
    ] {
        repository
            .upsert(UpsertUsageRecord {
                status: "failed".to_string(),
                billing_status: "void".to_string(),
                status_code: Some(503),
                finalized_at_unix_secs: Some(101),
                updated_at_unix_secs: 101,
                ..sample_upsert_usage_record(request_id)
            })
            .await
            .expect("failed usage should upsert");

        let stored = repository
            .upsert(UpsertUsageRecord {
                status: status.to_string(),
                billing_status: billing_status.to_string(),
                status_code,
                finalized_at_unix_secs: None,
                updated_at_unix_secs: 102,
                ..sample_upsert_usage_record(request_id)
            })
            .await
            .expect("non-authoritative recovery should be ignored");

        assert_eq!(stored.status, "failed");
        assert_eq!(stored.billing_status, "void");
        assert_eq!(stored.status_code, Some(503));
        assert_eq!(stored.finalized_at_unix_secs, Some(101));
    }
}

#[tokio::test]
async fn stale_pending_update_does_not_reopen_void_failure() {
    let repository = InMemoryUsageReadRepository::default();
    repository
        .upsert(UpsertUsageRecord {
            status: "failed".to_string(),
            billing_status: "void".to_string(),
            status_code: Some(503),
            error_message: Some("provider timeout".to_string()),
            error_category: Some("provider_error".to_string()),
            response_time_ms: Some(90),
            finalized_at_unix_secs: Some(101),
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
            ..sample_upsert_usage_record("req-void-failure-1")
        })
        .await
        .expect("failed usage should upsert");

    repository
        .upsert(UpsertUsageRecord {
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 102,
            ..sample_upsert_usage_record("req-void-failure-1")
        })
        .await
        .expect("stale pending usage should upsert");

    let stored = repository
        .find_by_request_id("req-void-failure-1")
        .await
        .expect("usage lookup should succeed")
        .expect("usage should exist");
    assert_eq!(stored.status, "failed");
    assert_eq!(stored.billing_status, "void");
    assert_eq!(stored.status_code, Some(503));
    assert_eq!(stored.finalized_at_unix_secs, Some(101));
}

#[tokio::test]
async fn stale_pending_update_does_not_regress_streaming_usage() {
    let repository = InMemoryUsageReadRepository::default();
    repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-streaming-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: Some("gpt-5-upstream".to_string()),
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(true),
            input_tokens: Some(10),
            output_tokens: Some(2),
            total_tokens: Some(12),
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.0),
            actual_total_cost_usd: Some(0.0),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(45),
            first_byte_time_ms: Some(12),
            status: "streaming".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: Some("cand-1".to_string()),
            candidate_index: Some(1),
            key_name: Some("primary".to_string()),
            planner_kind: Some("claude_cli_sync".to_string()),
            route_family: Some("claude".to_string()),
            route_kind: Some("cli".to_string()),
            execution_path: Some("remote".to_string()),
            local_execution_runtime_miss_reason: None,
            request_metadata: Some(json!({
                "trace_id": "trace-streaming"
            })),
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("streaming usage should upsert");

    repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-streaming-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(true),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: None,
            actual_total_cost_usd: None,
            status_code: None,
            error_message: None,
            error_category: None,
            response_time_ms: None,
            first_byte_time_ms: None,
            status: "pending".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 102,
        })
        .await
        .expect("stale pending usage should upsert");

    let stored = repository
        .find_by_request_id("req-streaming-1")
        .await
        .expect("usage lookup should succeed")
        .expect("usage should exist");
    assert_eq!(stored.status, "streaming");
    assert_eq!(stored.status_code, Some(200));
    assert_eq!(stored.first_byte_time_ms, Some(12));
    assert_eq!(stored.response_time_ms, Some(45));
    assert_eq!(stored.target_model.as_deref(), Some("gpt-5-upstream"));
    assert_eq!(stored.total_tokens, 12);
}

#[tokio::test]
async fn streaming_refresh_without_timing_does_not_clear_stream_timing() {
    let repository = InMemoryUsageReadRepository::default();
    let mut first = sample_upsert_usage_record("req-streaming-refresh");
    first.is_stream = Some(true);
    first.status = "streaming".to_string();
    first.status_code = Some(200);
    first.response_time_ms = Some(45);
    first.first_byte_time_ms = Some(12);
    repository
        .upsert(first)
        .await
        .expect("streaming usage should upsert");

    let mut refresh = sample_upsert_usage_record("req-streaming-refresh");
    refresh.is_stream = Some(true);
    refresh.status = "streaming".to_string();
    refresh.status_code = None;
    repository
        .upsert(refresh)
        .await
        .expect("streaming refresh should upsert");

    let stored = repository
        .find_by_request_id("req-streaming-refresh")
        .await
        .expect("usage lookup should succeed")
        .expect("usage should exist");
    assert_eq!(stored.status, "streaming");
    assert_eq!(stored.status_code, Some(200));
    assert_eq!(stored.response_time_ms, Some(45));
    assert_eq!(stored.first_byte_time_ms, Some(12));
}

#[tokio::test]
async fn seed_hydrates_legacy_body_ref_metadata_into_typed_fields() {
    let repository = InMemoryUsageReadRepository::seed(vec![StoredRequestUsageAudit {
        request_metadata: Some(json!({
            "request_body_ref": "usage://request/req-legacy/request_body"
        })),
        ..sample_usage("req-legacy", 100)
    }]);

    let usage = repository
        .find_by_request_id("req-legacy")
        .await
        .expect("find should succeed")
        .expect("usage should exist");

    assert_eq!(
        usage.body_ref(UsageBodyField::RequestBody),
        Some("usage://request/req-legacy/request_body")
    );
    assert_eq!(
        usage.request_metadata,
        Some(json!({
            "request_body_ref": "usage://request/req-legacy/request_body"
        }))
    );
}

#[tokio::test]
async fn seed_ignores_invalid_or_mismatched_legacy_body_ref_metadata() {
    let repository = InMemoryUsageReadRepository::seed(vec![
        StoredRequestUsageAudit {
            request_metadata: Some(json!({
                "request_body_ref": "blob://legacy-request"
            })),
            ..sample_usage("req-invalid-legacy", 100)
        },
        StoredRequestUsageAudit {
            request_metadata: Some(json!({
                "request_body_ref": "usage://request/req-other/request_body"
            })),
            ..sample_usage("req-mismatched-legacy", 200)
        },
    ]);

    let invalid = repository
        .find_by_request_id("req-invalid-legacy")
        .await
        .expect("find should succeed")
        .expect("usage should exist");
    let mismatched = repository
        .find_by_request_id("req-mismatched-legacy")
        .await
        .expect("find should succeed")
        .expect("usage should exist");

    assert_eq!(invalid.body_ref(UsageBodyField::RequestBody), None);
    assert_eq!(mismatched.body_ref(UsageBodyField::RequestBody), None);
}

#[tokio::test]
async fn detached_body_seed_moves_large_payloads_behind_usage_refs() {
    let mut usage = sample_usage("req-detached", 100);
    usage.request_body = Some(json!({
        "model": "gpt-4.1",
        "messages": [{"role": "user", "content": "hello"}]
    }));
    usage.provider_request_body = Some(json!({
        "model": "gpt-4.1-mini",
        "stream": false
    }));

    let repository = InMemoryUsageReadRepository::seed_with_detached_bodies(vec![usage]);

    let stored = repository
        .find_by_request_id("req-detached")
        .await
        .expect("find should succeed")
        .expect("usage should exist");

    assert!(stored.request_body.is_none());
    assert!(stored.provider_request_body.is_none());
    assert_eq!(
        stored.body_ref(UsageBodyField::RequestBody),
        Some("usage://request/req-detached/request_body")
    );
    assert_eq!(
        stored.body_ref(UsageBodyField::ProviderRequestBody),
        Some("usage://request/req-detached/provider_request_body")
    );
    assert_eq!(stored.request_metadata, None);
    assert_eq!(
        repository
            .resolve_body_ref(&usage_body_ref("req-detached", UsageBodyField::RequestBody))
            .await
            .expect("body ref should resolve"),
        Some(json!({
            "model": "gpt-4.1",
            "messages": [{"role": "user", "content": "hello"}]
        }))
    );
    assert_eq!(
        repository
            .resolve_body_ref(&usage_body_ref(
                "req-detached",
                UsageBodyField::ProviderRequestBody
            ))
            .await
            .expect("provider request body ref should resolve"),
        Some(json!({
            "model": "gpt-4.1-mini",
            "stream": false
        }))
    );
}

#[tokio::test]
async fn seed_discards_cross_request_and_cross_field_body_refs() {
    let mut usage = sample_usage("req-ref-target", 100);
    usage.request_body_ref = Some("usage://request/req-ref-owner/request_body".to_string());
    usage.response_body_ref = Some("usage://request/req-ref-target/request_body".to_string());

    let repository = InMemoryUsageReadRepository::seed(vec![usage]);
    let stored = repository
        .find_by_request_id("req-ref-target")
        .await
        .expect("find should succeed")
        .expect("usage should exist");

    assert!(stored.request_body_ref.is_none());
    assert!(stored.response_body_ref.is_none());
}

#[tokio::test]
async fn upsert_writes_usage_record() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-upsert-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: Some("gpt-5-mini".to_string()),
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(true),
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.25),
            actual_total_cost_usd: Some(0.15),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(300),
            first_byte_time_ms: Some(120),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: Some(json!({"authorization": "Bearer test"})),
            request_body: Some(json!({"model": "gpt-5"})),
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("upsert should succeed");

    assert_eq!(stored.request_id, "req-upsert-1");
    assert_eq!(stored.total_tokens, 30);
    assert_eq!(stored.total_cost_usd, 0.25);
    assert_eq!(stored.actual_total_cost_usd, 0.15);
    assert_eq!(
        repository
            .find_by_request_id("req-upsert-1")
            .await
            .expect("find should succeed")
            .expect("usage should exist")
            .model,
        "gpt-5"
    );
}

#[tokio::test]
async fn upsert_defaults_created_at_to_second_timestamp() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-upsert-ms-default".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: None,
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: None,
            api_format: None,
            api_family: None,
            endpoint_kind: None,
            endpoint_api_format: None,
            provider_api_family: None,
            provider_endpoint_kind: None,
            has_format_conversion: None,
            is_stream: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: None,
            actual_total_cost_usd: None,
            status_code: None,
            error_message: None,
            error_category: None,
            response_time_ms: None,
            first_byte_time_ms: None,
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: None,
            updated_at_unix_secs: 101,
        })
        .await
        .expect("upsert should succeed");

    assert_eq!(stored.created_at_unix_ms, 101);
}

#[tokio::test]
async fn upsert_does_not_backfill_legacy_output_price_from_request_metadata() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-upsert-price-metadata".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: None,
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.25),
            actual_total_cost_usd: Some(0.15),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(300),
            first_byte_time_ms: Some(120),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: Some(json!({
                "output_price_per_1m": 15.0
            })),
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("upsert should succeed");

    assert_eq!(stored.output_price_per_1m, None);
    assert_eq!(stored.settlement_output_price_per_1m(), Some(15.0));
}

#[tokio::test]
async fn upsert_does_not_backfill_typed_body_refs_from_request_metadata() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-upsert-body-ref-metadata".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: None,
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.25),
            actual_total_cost_usd: Some(0.15),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(300),
            first_byte_time_ms: Some(120),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: Some(json!({
                "request_body_ref": "usage://request/req-upsert-body-ref-metadata/request_body"
            })),
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("upsert should succeed");

    assert_eq!(stored.request_body_ref, None);
    assert_eq!(stored.request_metadata, None);
}

#[tokio::test]
async fn upsert_keeps_typed_routing_fields_out_of_request_metadata() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-upsert-routing-metadata".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(true),
            is_stream: Some(false),
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.25),
            actual_total_cost_usd: Some(0.15),
            status_code: Some(503),
            error_message: None,
            error_category: None,
            response_time_ms: Some(300),
            first_byte_time_ms: Some(120),
            status: "failed".to_string(),
            billing_status: "void".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: Some("cand-typed".to_string()),
            candidate_index: Some(2),
            key_name: Some("primary".to_string()),
            planner_kind: Some("claude_cli_sync".to_string()),
            route_family: Some("claude".to_string()),
            route_kind: Some("cli".to_string()),
            execution_path: Some("local_execution_runtime_miss".to_string()),
            local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
            request_metadata: Some(json!({
                "trace_id": "trace-1"
            })),
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("upsert should succeed");

    assert_eq!(
        stored.request_metadata,
        Some(json!({ "trace_id": "trace-1" }))
    );
    assert_eq!(stored.routing_candidate_id(), Some("cand-typed"));
    assert_eq!(stored.routing_candidate_index(), Some(2));
    assert_eq!(stored.routing_key_name(), Some("primary"));
    assert_eq!(stored.routing_planner_kind(), Some("claude_cli_sync"));
    assert_eq!(stored.routing_route_family(), Some("claude"));
    assert_eq!(stored.routing_route_kind(), Some("cli"));
    assert_eq!(
        stored.routing_execution_path(),
        Some("local_execution_runtime_miss")
    );
    assert_eq!(
        stored.routing_local_execution_runtime_miss_reason(),
        Some("all_candidates_skipped")
    );
}

#[tokio::test]
async fn upsert_does_not_persist_legacy_display_columns_for_new_rows() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-upsert-display-columns".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("alice".to_string()),
            api_key_name: Some("default".to_string()),
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: None,
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.25),
            actual_total_cost_usd: Some(0.15),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(300),
            first_byte_time_ms: Some(120),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await
        .expect("upsert should succeed");

    assert_eq!(stored.username, None);
    assert_eq!(stored.api_key_name, None);
}

#[tokio::test]
async fn upsert_preserves_existing_legacy_display_columns_when_new_write_omits_them() {
    let repository = InMemoryUsageReadRepository::seed(vec![StoredRequestUsageAudit {
        id: "usage-req-existing-display-columns".to_string(),
        request_id: "req-existing-display-columns".to_string(),
        user_id: Some("user-1".to_string()),
        api_key_id: Some("key-1".to_string()),
        username: Some("legacy-alice".to_string()),
        api_key_name: Some("legacy-default".to_string()),
        provider_name: "OpenAI".to_string(),
        model: "gpt-5".to_string(),
        target_model: None,
        provider_id: None,
        provider_endpoint_id: None,
        provider_api_key_id: None,
        request_type: Some("chat".to_string()),
        api_format: Some("openai:chat".to_string()),
        api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_api_format: Some("openai:chat".to_string()),
        provider_api_family: Some("openai".to_string()),
        provider_endpoint_kind: Some("chat".to_string()),
        has_format_conversion: false,
        is_stream: false,
        client_family: None,
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        cache_creation_input_tokens: 0,
        cache_creation_ephemeral_5m_input_tokens: 0,
        cache_creation_ephemeral_1h_input_tokens: 0,
        cache_read_input_tokens: 0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        output_price_per_1m: None,
        total_cost_usd: 0.25,
        actual_total_cost_usd: 0.15,
        status_code: Some(200),
        error_message: None,
        error_category: None,
        response_time_ms: Some(300),
        first_byte_time_ms: Some(120),
        status: "completed".to_string(),
        billing_status: "pending".to_string(),
        request_headers: None,
        request_body: None,
        request_body_ref: None,
        provider_request_headers: None,
        provider_request_body: None,
        provider_request_body_ref: None,
        response_headers: None,
        response_body: None,
        response_body_ref: None,
        client_response_headers: None,
        client_response_body: None,
        client_response_body_ref: None,
        request_body_state: None,
        provider_request_body_state: None,
        response_body_state: None,
        client_response_body_state: None,
        candidate_id: None,
        candidate_index: None,
        key_name: None,
        planner_kind: None,
        route_family: None,
        route_kind: None,
        execution_path: None,
        local_execution_runtime_miss_reason: None,
        request_metadata: None,
        created_at_unix_ms: 100,
        updated_at_unix_secs: 101,
        finalized_at_unix_secs: None,
    }]);
    let stored = repository
        .upsert(UpsertUsageRecord {
            capture_retention: Default::default(),
            request_id: "req-existing-display-columns".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("fresh-alice".to_string()),
            api_key_name: Some("fresh-default".to_string()),
            provider_name: "OpenAI".to_string(),
            model: "gpt-5-mini".to_string(),
            target_model: None,
            provider_id: None,
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(30),
            output_tokens: Some(40),
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.45),
            actual_total_cost_usd: Some(0.30),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(200),
            first_byte_time_ms: Some(80),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 102,
        })
        .await
        .expect("upsert should succeed");

    assert_eq!(stored.username.as_deref(), Some("legacy-alice"));
    assert_eq!(stored.api_key_name.as_deref(), Some("legacy-default"));
    assert_eq!(stored.model, "gpt-5-mini");
}

#[tokio::test]
async fn summarizes_provider_usage_windows_since_timestamp() {
    let repository = InMemoryUsageReadRepository::default().with_provider_usage_windows(vec![
        StoredProviderUsageWindow::new(
            "provider-1".to_string(),
            1_700_000_000,
            10,
            9,
            1,
            120.0,
            1.25,
        )
        .expect("window should build"),
        StoredProviderUsageWindow::new(
            "provider-1".to_string(),
            1_700_003_600,
            6,
            5,
            1,
            180.0,
            0.75,
        )
        .expect("window should build"),
        StoredProviderUsageWindow::new(
            "provider-2".to_string(),
            1_700_003_600,
            99,
            99,
            0,
            50.0,
            5.0,
        )
        .expect("window should build"),
    ]);

    let summary = repository
        .summarize_provider_usage_since("provider-1", 1_700_000_100)
        .await
        .expect("summary should succeed");

    assert_eq!(summary.total_requests, 6);
    assert_eq!(summary.successful_requests, 5);
    assert_eq!(summary.failed_requests, 1);
    assert_eq!(summary.avg_response_time_ms, 180.0);
    assert_eq!(summary.total_cost_usd, 0.75);
}

#[tokio::test]
async fn summarizes_usage_by_provider_api_key_ids() {
    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-1", 1_711_000_000),
        sample_usage("req-2", 1_711_000_250),
    ]);

    let usage = repository
        .summarize_usage_by_provider_api_key_ids(&["provider-key-1".to_string()])
        .await
        .expect("summary should succeed");
    let item = usage
        .get("provider-key-1")
        .expect("provider key summary should exist");

    assert_eq!(item.request_count, 2);
    assert_eq!(item.total_tokens, 300);
    assert_eq!(item.total_cost_usd, 0.24);
    assert_eq!(item.last_used_at_unix_secs, Some(1_711_000_250));
}

#[tokio::test]
async fn summarizes_provider_api_key_window_usage_with_zero_rows() {
    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-1", 1_711_000_000),
        sample_usage("req-2", 1_711_000_250),
    ]);

    let usage = repository
        .summarize_usage_by_provider_api_key_windows(&[
            ProviderApiKeyWindowUsageRequest {
                provider_api_key_id: "provider-key-1".to_string(),
                window_code: "5h".to_string(),
                start_unix_secs: 1_711_000_000,
                end_unix_secs: 1_711_000_300,
            },
            ProviderApiKeyWindowUsageRequest {
                provider_api_key_id: "provider-key-empty".to_string(),
                window_code: "weekly".to_string(),
                start_unix_secs: 1_711_000_000,
                end_unix_secs: 1_711_000_300,
            },
        ])
        .await
        .expect("window summary should succeed");

    assert_eq!(usage.len(), 2);
    assert_eq!(usage[0].provider_api_key_id, "provider-key-1");
    assert_eq!(usage[0].window_code, "5h");
    assert_eq!(usage[0].request_count, 2);
    assert_eq!(usage[0].total_tokens, 300);
    assert_eq!(usage[0].total_cost_usd, 0.24);
    assert_eq!(usage[1].provider_api_key_id, "provider-key-empty");
    assert_eq!(usage[1].window_code, "weekly");
    assert_eq!(usage[1].request_count, 0);
    assert_eq!(usage[1].total_tokens, 0);
    assert_eq!(usage[1].total_cost_usd, 0.0);
}

#[tokio::test]
async fn list_usage_audits_applies_second_based_time_filters() {
    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-1", 1),
        sample_usage("req-2", 2),
        sample_usage("req-3", 3),
    ]);

    let items = repository
        .list_usage_audits(&crate::repository::usage::UsageAuditListQuery {
            created_from_unix_secs: Some(2),
            created_until_unix_secs: Some(3),
            ..Default::default()
        })
        .await
        .expect("list should succeed");

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].request_id, "req-2");
}

#[tokio::test]
async fn usage_audit_websocket_filter_applies_to_list_count_and_keyword_search() {
    let mut websocket = sample_usage("req-ws", 2);
    websocket.request_metadata = Some(json!({
        "websocket_mode": true,
        "websocket_transport": "codex_live_direct",
    }));
    let repository =
        InMemoryUsageReadRepository::seed(vec![sample_usage("req-http", 1), websocket]);

    let list_query = crate::repository::usage::UsageAuditListQuery {
        is_websocket: Some(true),
        ..Default::default()
    };
    let listed = repository
        .list_usage_audits(&list_query)
        .await
        .expect("WebSocket list should succeed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].request_id, "req-ws");
    assert_eq!(
        repository
            .count_usage_audits(&list_query)
            .await
            .expect("WebSocket count should succeed"),
        1
    );

    let keyword_query = UsageAuditKeywordSearchQuery {
        is_websocket: Some(true),
        keywords: vec!["gpt-4.1".to_string()],
        ..Default::default()
    };
    let keyword_matches = repository
        .list_usage_audits_by_keyword_search(&keyword_query)
        .await
        .expect("WebSocket keyword list should succeed");
    assert_eq!(keyword_matches.len(), 1);
    assert_eq!(keyword_matches[0].request_id, "req-ws");
    assert_eq!(
        repository
            .count_usage_audits_by_keyword_search(&keyword_query)
            .await
            .expect("WebSocket keyword count should succeed"),
        1
    );
}

#[tokio::test]
async fn dashboard_and_leaderboard_total_tokens_use_effective_cache_aware_tokens() {
    let mut item = sample_usage("req-cache-aware-total", 1_711_000_000);
    item.input_tokens = 100;
    item.output_tokens = 20;
    item.total_tokens = 999;
    item.cache_creation_input_tokens = 0;
    item.cache_creation_ephemeral_5m_input_tokens = 12;
    item.cache_creation_ephemeral_1h_input_tokens = 8;
    item.cache_read_input_tokens = 80;

    let repository = InMemoryUsageReadRepository::seed(vec![item]);

    let dashboard = repository
        .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
            created_from_unix_secs: 1_711_000_000,
            created_until_unix_secs: 1_711_000_001,
            user_id: None,
        })
        .await
        .expect("dashboard should summarize");
    assert_eq!(dashboard.effective_input_tokens, 0);
    assert_eq!(dashboard.cache_creation_tokens, 20);
    assert_eq!(dashboard.total_tokens, 120);

    let leaderboard = repository
        .summarize_usage_leaderboard(&UsageLeaderboardQuery {
            provider_names: None,
            created_from_unix_secs: 1_711_000_000,
            created_until_unix_secs: 1_711_000_001,
            group_by: UsageLeaderboardGroupBy::User,
            user_id: None,
            user_ids: None,
            provider_name: None,
            model: None,
        })
        .await
        .expect("leaderboard should summarize");
    assert_eq!(leaderboard.len(), 1);
    assert_eq!(leaderboard[0].total_tokens, 120);
}

#[tokio::test]
async fn usage_analytics_filters_by_multiple_user_ids() {
    let user_one = sample_usage("req-user-1", 1_711_000_000);
    let mut user_two = sample_usage("req-user-2", 1_711_000_000);
    user_two.user_id = Some("user-2".to_string());
    let mut user_three = sample_usage("req-user-3", 1_711_000_000);
    user_three.user_id = Some("user-3".to_string());
    let repository = InMemoryUsageReadRepository::seed(vec![user_one, user_two, user_three]);
    let scoped_user_ids = vec!["user-1".to_string(), "user-2".to_string()];

    let summary = repository
        .summarize_usage_audits(&UsageAuditSummaryQuery {
            provider_names: None,
            created_from_unix_secs: 1_711_000_000,
            created_until_unix_secs: 1_711_000_001,
            user_ids: Some(scoped_user_ids.clone()),
            ..Default::default()
        })
        .await
        .expect("summary should filter by multiple users");
    assert_eq!(summary.total_requests, 2);

    let buckets = repository
        .summarize_usage_time_series(&UsageTimeSeriesQuery {
            provider_names: None,
            created_from_unix_secs: 1_711_000_000,
            created_until_unix_secs: 1_711_000_001,
            granularity: UsageTimeSeriesGranularity::Day,
            tz_offset_minutes: 0,
            user_id: None,
            user_ids: Some(scoped_user_ids.clone()),
            provider_name: None,
            model: None,
        })
        .await
        .expect("time series should filter by multiple users");
    assert_eq!(
        buckets
            .iter()
            .map(|bucket| bucket.total_requests)
            .sum::<u64>(),
        2
    );

    let leaderboard = repository
        .summarize_usage_leaderboard(&UsageLeaderboardQuery {
            provider_names: None,
            created_from_unix_secs: 1_711_000_000,
            created_until_unix_secs: 1_711_000_001,
            group_by: UsageLeaderboardGroupBy::User,
            user_id: None,
            user_ids: Some(scoped_user_ids),
            provider_name: None,
            model: None,
        })
        .await
        .expect("leaderboard should filter by multiple users");
    assert_eq!(leaderboard.len(), 2);
    assert!(leaderboard.iter().all(|item| item.group_key != "user-3"));
}

#[tokio::test]
async fn usage_analytics_intersects_provider_allowlist_and_user_scope() {
    let mut a = sample_usage("allowed", 1_711_000_000);
    a.provider_name = "Gemini".to_string();
    a.user_id = Some("user-1".to_string());
    let mut b = a.clone();
    b.request_id = "other-provider".to_string();
    b.provider_name = "Other".to_string();
    let mut c = a.clone();
    c.request_id = "other-user".to_string();
    c.user_id = Some("user-2".to_string());
    let repository = InMemoryUsageReadRepository::seed(vec![a, b, c]);
    for (names, provider, count) in [
        (
            Some(vec!["Gemini".to_string(), "Gemini".to_string()]),
            None,
            1,
        ),
        (Some(vec![]), None, 0),
        (
            Some(vec!["Gemini".to_string()]),
            Some("Other".to_string()),
            0,
        ),
        (None, None, 2),
    ] {
        let summary = repository
            .summarize_usage_audits(&UsageAuditSummaryQuery {
                created_from_unix_secs: 1_711_000_000,
                created_until_unix_secs: 1_711_000_001,
                user_ids: Some(vec!["user-1".to_string()]),
                provider_names: names.clone(),
                provider_name: provider.clone(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(summary.total_requests, count);
        let buckets = repository
            .summarize_usage_time_series(&UsageTimeSeriesQuery {
                created_from_unix_secs: 1_711_000_000,
                created_until_unix_secs: 1_711_000_001,
                user_id: None,
                user_ids: Some(vec!["user-1".to_string()]),
                provider_names: names.clone(),
                provider_name: provider.clone(),
                model: None,
                granularity: UsageTimeSeriesGranularity::Day,
                tz_offset_minutes: 0,
            })
            .await
            .unwrap();
        assert_eq!(
            buckets.iter().map(|row| row.total_requests).sum::<u64>(),
            count
        );
        let rows = repository
            .summarize_usage_leaderboard(&UsageLeaderboardQuery {
                created_from_unix_secs: 1_711_000_000,
                created_until_unix_secs: 1_711_000_001,
                user_id: None,
                user_ids: Some(vec!["user-1".to_string()]),
                provider_names: names,
                provider_name: provider,
                model: None,
                group_by: UsageLeaderboardGroupBy::User,
            })
            .await
            .unwrap();
        assert_eq!(rows.iter().map(|row| row.request_count).sum::<u64>(), count);
    }
}

#[tokio::test]
async fn summarizes_provider_api_key_last_used_at_in_seconds() {
    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-1", 1_999),
        sample_usage("req-2", 2_500),
    ]);

    let summary = repository
        .summarize_usage_by_provider_api_key_ids(&["provider-key-1".to_string()])
        .await
        .expect("summary should succeed");

    let usage = summary
        .get("provider-key-1")
        .expect("provider key summary should exist");
    assert_eq!(usage.request_count, 2);
    assert_eq!(usage.last_used_at_unix_secs, Some(2_500));
}

fn sample_provider_catalog_key(key_id: &str) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        key_id.to_string(),
        "provider-1".to_string(),
        "provider key".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("provider key should build")
}

fn sample_provider_catalog_repository(
    key_ids: &[&str],
) -> Arc<InMemoryProviderCatalogReadRepository> {
    Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "OpenAI".to_string(),
            None,
            "openai".to_string(),
        )
        .expect("provider should build")],
        Vec::new(),
        key_ids
            .iter()
            .map(|key_id| sample_provider_catalog_key(key_id))
            .collect(),
    ))
}

fn sample_auth_api_key_repository(
    api_key_ids: &[&str],
) -> Arc<InMemoryAuthApiKeySnapshotRepository> {
    let snapshots = api_key_ids.iter().map(|api_key_id| {
        (
            Some(format!("hash-{api_key_id}")),
            StoredAuthApiKeySnapshot::new(
                "user-1".to_string(),
                "alice".to_string(),
                Some("alice@example.com".to_string()),
                "user".to_string(),
                "local".to_string(),
                true,
                false,
                None,
                None,
                None,
                (*api_key_id).to_string(),
                Some(format!("Key {api_key_id}")),
                true,
                false,
                false,
                Some(120),
                Some(8),
                None,
                None,
                None,
                None,
            )
            .expect("snapshot should build"),
        )
    });
    let export_records = api_key_ids.iter().map(|api_key_id| {
        StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            (*api_key_id).to_string(),
            format!("hash-{api_key_id}"),
            Some(format!("enc-{api_key_id}")),
            Some(format!("Key {api_key_id}")),
            None,
            None,
            None,
            Some(120),
            Some(8),
            None,
            true,
            None,
            false,
            0,
            0,
            0.0,
            false,
        )
        .expect("export record should build")
    });
    Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(snapshots).with_export_records(export_records),
    )
}

#[tokio::test]
async fn upsert_syncs_linked_provider_key_stats_without_double_counting_request_count() {
    let provider_catalog = sample_provider_catalog_repository(&["provider-key-1"]);
    let repository = InMemoryUsageReadRepository::default()
        .with_provider_catalog_repository(Arc::clone(&provider_catalog));

    repository
        .upsert(UpsertUsageRecord {
            provider_api_key_id: Some("provider-key-1".to_string()),
            total_tokens: Some(100),
            total_cost_usd: Some(0.5),
            created_at_unix_ms: Some(1_711_100_000),
            updated_at_unix_secs: 1_711_100_000,
            ..sample_upsert_usage_record("req-linked-1")
        })
        .await
        .expect("pending upsert should succeed");
    repository
        .upsert(UpsertUsageRecord {
            provider_api_key_id: Some("provider-key-1".to_string()),
            status: "completed".to_string(),
            billing_status: "settled".to_string(),
            status_code: Some(200),
            response_time_ms: Some(240),
            total_tokens: Some(180),
            total_cost_usd: Some(0.75),
            created_at_unix_ms: Some(1_711_100_000),
            updated_at_unix_secs: 1_711_100_010,
            finalized_at_unix_secs: Some(1_711_100_011),
            ..sample_upsert_usage_record("req-linked-1")
        })
        .await
        .expect("completed upsert should succeed");

    let key = provider_catalog
        .list_keys_by_ids(&["provider-key-1".to_string()])
        .await
        .expect("key list should succeed")
        .into_iter()
        .next()
        .expect("provider key should exist");
    assert_eq!(key.request_count, Some(1));
    assert_eq!(key.success_count, Some(1));
    assert_eq!(key.error_count, Some(0));
    assert_eq!(key.total_tokens, 180);
    assert_eq!(key.total_cost_usd, 0.75);
    assert_eq!(key.total_response_time_ms, Some(240));
    assert_eq!(key.last_used_at_unix_secs, Some(1_711_100_000));
}

#[tokio::test]
async fn upsert_syncs_linked_api_key_stats_without_double_counting_request_count() {
    let auth_api_keys = sample_auth_api_key_repository(&["api-key-1"]);
    let repository = InMemoryUsageReadRepository::default()
        .with_auth_api_key_repository(Arc::clone(&auth_api_keys));

    repository
        .upsert(UpsertUsageRecord {
            api_key_id: Some("api-key-1".to_string()),
            total_tokens: Some(100),
            total_cost_usd: Some(0.5),
            created_at_unix_ms: Some(1_711_100_000),
            updated_at_unix_secs: 1_711_100_000,
            ..sample_upsert_usage_record("req-api-key-1")
        })
        .await
        .expect("pending upsert should succeed");
    repository
        .upsert(UpsertUsageRecord {
            api_key_id: Some("api-key-1".to_string()),
            status: "completed".to_string(),
            billing_status: "settled".to_string(),
            total_tokens: Some(180),
            total_cost_usd: Some(0.75),
            created_at_unix_ms: Some(1_711_100_000),
            updated_at_unix_secs: 1_711_100_010,
            finalized_at_unix_secs: Some(1_711_100_011),
            ..sample_upsert_usage_record("req-api-key-1")
        })
        .await
        .expect("completed upsert should succeed");

    let key = auth_api_keys
        .list_export_api_keys_by_ids(&["api-key-1".to_string()])
        .await
        .expect("key list should succeed")
        .into_iter()
        .next()
        .expect("api key should exist");
    assert_eq!(key.total_requests, 1);
    assert_eq!(key.total_tokens, 180);
    assert_eq!(key.total_cost_usd, 0.75);
}

#[tokio::test]
async fn upsert_moves_linked_provider_key_stats_when_key_assignment_changes() {
    let provider_catalog =
        sample_provider_catalog_repository(&["provider-key-a", "provider-key-b"]);
    let repository = InMemoryUsageReadRepository::default()
        .with_provider_catalog_repository(Arc::clone(&provider_catalog));

    repository
        .upsert(UpsertUsageRecord {
            provider_api_key_id: Some("provider-key-a".to_string()),
            status: "completed".to_string(),
            billing_status: "settled".to_string(),
            status_code: Some(200),
            response_time_ms: Some(100),
            total_tokens: Some(120),
            total_cost_usd: Some(0.4),
            created_at_unix_ms: Some(1_711_200_000),
            updated_at_unix_secs: 1_711_200_000,
            finalized_at_unix_secs: Some(1_711_200_001),
            ..sample_upsert_usage_record("req-move-1")
        })
        .await
        .expect("first upsert should succeed");
    repository
        .upsert(UpsertUsageRecord {
            provider_api_key_id: Some("provider-key-b".to_string()),
            status: "completed".to_string(),
            billing_status: "settled".to_string(),
            status_code: Some(200),
            response_time_ms: Some(150),
            total_tokens: Some(140),
            total_cost_usd: Some(0.6),
            created_at_unix_ms: Some(1_711_200_000),
            updated_at_unix_secs: 1_711_200_010,
            finalized_at_unix_secs: Some(1_711_200_011),
            ..sample_upsert_usage_record("req-move-1")
        })
        .await
        .expect("moved upsert should succeed");

    let keys = provider_catalog
        .list_keys_by_ids(&["provider-key-a".to_string(), "provider-key-b".to_string()])
        .await
        .expect("key list should succeed");
    let key_a = keys
        .iter()
        .find(|key| key.id == "provider-key-a")
        .expect("key a should exist");
    let key_b = keys
        .iter()
        .find(|key| key.id == "provider-key-b")
        .expect("key b should exist");

    assert_eq!(key_a.request_count, Some(0));
    assert_eq!(key_a.success_count, Some(0));
    assert_eq!(key_a.total_tokens, 0);
    assert_eq!(key_a.total_cost_usd, 0.0);
    assert_eq!(key_a.total_response_time_ms, Some(0));
    assert_eq!(key_a.last_used_at_unix_secs, None);

    assert_eq!(key_b.request_count, Some(1));
    assert_eq!(key_b.success_count, Some(1));
    assert_eq!(key_b.total_tokens, 140);
    assert_eq!(key_b.total_cost_usd, 0.6);
    assert_eq!(key_b.total_response_time_ms, Some(150));
    assert_eq!(key_b.last_used_at_unix_secs, Some(1_711_200_000));
}

#[tokio::test]
async fn upsert_moves_linked_api_key_stats_when_key_assignment_changes() {
    let auth_api_keys = sample_auth_api_key_repository(&["api-key-a", "api-key-b"]);
    let repository = InMemoryUsageReadRepository::default()
        .with_auth_api_key_repository(Arc::clone(&auth_api_keys));

    repository
        .upsert(UpsertUsageRecord {
            api_key_id: Some("api-key-a".to_string()),
            status: "completed".to_string(),
            billing_status: "settled".to_string(),
            total_tokens: Some(120),
            total_cost_usd: Some(0.4),
            created_at_unix_ms: Some(1_711_200_000),
            updated_at_unix_secs: 1_711_200_000,
            finalized_at_unix_secs: Some(1_711_200_001),
            ..sample_upsert_usage_record("req-api-move-1")
        })
        .await
        .expect("first upsert should succeed");
    repository
        .upsert(UpsertUsageRecord {
            api_key_id: Some("api-key-b".to_string()),
            status: "completed".to_string(),
            billing_status: "settled".to_string(),
            total_tokens: Some(140),
            total_cost_usd: Some(0.6),
            created_at_unix_ms: Some(1_711_200_000),
            updated_at_unix_secs: 1_711_200_010,
            finalized_at_unix_secs: Some(1_711_200_011),
            ..sample_upsert_usage_record("req-api-move-1")
        })
        .await
        .expect("moved upsert should succeed");

    let keys = auth_api_keys
        .list_export_api_keys_by_ids(&["api-key-a".to_string(), "api-key-b".to_string()])
        .await
        .expect("key list should succeed");
    let key_a = keys
        .iter()
        .find(|key| key.api_key_id == "api-key-a")
        .expect("key a should exist");
    let key_b = keys
        .iter()
        .find(|key| key.api_key_id == "api-key-b")
        .expect("key b should exist");

    assert_eq!(key_a.total_requests, 0);
    assert_eq!(key_a.total_tokens, 0);
    assert_eq!(key_a.total_cost_usd, 0.0);

    assert_eq!(key_b.total_requests, 1);
    assert_eq!(key_b.total_tokens, 140);
    assert_eq!(key_b.total_cost_usd, 0.6);
}

#[tokio::test]
async fn rebuild_provider_key_usage_stats_resets_linked_catalog_to_current_usage() {
    let provider_catalog = sample_provider_catalog_repository(&["provider-key-1"]);
    let mut stale_key = provider_catalog
        .list_keys_by_ids(&["provider-key-1".to_string()])
        .await
        .expect("key list should succeed")
        .into_iter()
        .next()
        .expect("provider key should exist");
    stale_key.request_count = Some(99);
    stale_key.success_count = Some(88);
    stale_key.error_count = Some(11);
    stale_key.total_tokens = 9_999;
    stale_key.total_cost_usd = 42.0;
    stale_key.total_response_time_ms = Some(9_999);
    stale_key.last_used_at_unix_secs = Some(9_999);
    provider_catalog
        .update_key(&stale_key)
        .await
        .expect("stale key should update");

    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-1", 1_711_300_000),
        sample_usage("req-2", 1_711_300_250),
    ])
    .with_provider_catalog_repository(Arc::clone(&provider_catalog));

    let rebuilt = repository
        .rebuild_provider_api_key_usage_stats()
        .await
        .expect("rebuild should succeed");
    assert_eq!(rebuilt, 1);

    let key = provider_catalog
        .list_keys_by_ids(&["provider-key-1".to_string()])
        .await
        .expect("key list should succeed")
        .into_iter()
        .next()
        .expect("provider key should exist");
    assert_eq!(key.request_count, Some(2));
    assert_eq!(key.success_count, Some(2));
    assert_eq!(key.error_count, Some(0));
    assert_eq!(key.total_tokens, 300);
    assert_eq!(key.total_cost_usd, 0.24);
    assert_eq!(key.total_response_time_ms, Some(840));
    assert_eq!(key.last_used_at_unix_secs, Some(1_711_300_250));
}

#[tokio::test]
async fn rebuild_api_key_usage_stats_resets_linked_auth_export_records_to_current_usage() {
    let auth_api_keys = sample_auth_api_key_repository(&["api-key-1"]);
    let mut stale_key = auth_api_keys
        .list_export_api_keys_by_ids(&["api-key-1".to_string()])
        .await
        .expect("key list should succeed")
        .into_iter()
        .next()
        .expect("api key should exist");
    stale_key.total_requests = 99;
    stale_key.total_tokens = 9_999;
    stale_key.total_cost_usd = 42.0;
    let auth_api_keys = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-api-key-1".to_string()),
            StoredAuthApiKeySnapshot::new(
                "user-1".to_string(),
                "alice".to_string(),
                Some("alice@example.com".to_string()),
                "user".to_string(),
                "local".to_string(),
                true,
                false,
                None,
                None,
                None,
                "api-key-1".to_string(),
                Some("Key api-key-1".to_string()),
                true,
                false,
                false,
                Some(120),
                Some(8),
                None,
                None,
                None,
                None,
            )
            .expect("snapshot should build"),
        )])
        .with_export_records(vec![stale_key]),
    );

    let repository = InMemoryUsageReadRepository::seed(vec![
        sample_usage("req-1", 1_711_300_000),
        sample_usage("req-2", 1_711_300_250),
    ])
    .with_auth_api_key_repository(Arc::clone(&auth_api_keys));

    let rebuilt = repository
        .rebuild_api_key_usage_stats()
        .await
        .expect("rebuild should succeed");
    assert_eq!(rebuilt, 1);

    let key = auth_api_keys
        .list_export_api_keys_by_ids(&["api-key-1".to_string()])
        .await
        .expect("key list should succeed")
        .into_iter()
        .next()
        .expect("api key should exist");
    assert_eq!(key.total_requests, 2);
    assert_eq!(key.total_tokens, 300);
    assert_eq!(key.total_cost_usd, 0.24);
}

#[tokio::test]
async fn summarize_usage_provider_performance_computes_tps_and_top_provider_timeline() {
    let mut first = sample_usage("req-provider-perf-1", 1_711_000_000);
    first.output_tokens = 60;
    first.response_time_ms = Some(3000);
    first.first_byte_time_ms = Some(100);

    let mut second = sample_usage("req-provider-perf-2", 1_711_000_300);
    second.output_tokens = 40;
    second.response_time_ms = Some(1000);
    second.first_byte_time_ms = Some(200);
    second.request_metadata = Some(json!({ "upstream_is_stream": true }));

    let mut failed = sample_usage("req-provider-perf-failed", 1_711_000_400);
    failed.output_tokens = 999;
    failed.response_time_ms = Some(10);
    failed.first_byte_time_ms = Some(1);
    failed.status = "failed".to_string();
    failed.status_code = Some(500);

    let mut other_provider = sample_usage("req-provider-perf-other", 1_711_003_600);
    other_provider.provider_id = Some("provider-2".to_string());
    other_provider.provider_name = "Anthropic".to_string();
    other_provider.output_tokens = 30;
    other_provider.response_time_ms = Some(3000);
    other_provider.first_byte_time_ms = None;

    let repository = InMemoryUsageReadRepository::seed(vec![first, second, failed, other_provider]);
    let query = UsageProviderPerformanceQuery {
        created_from_unix_secs: 1_711_000_000,
        created_until_unix_secs: 1_711_010_000,
        granularity: UsageTimeSeriesGranularity::Hour,
        tz_offset_minutes: 0,
        limit: 1,
        provider_id: None,
        model: None,
        api_format: None,
        endpoint_kind: None,
        is_stream: None,
        has_format_conversion: None,
        slow_threshold_ms: 10_000,
        include_timeline: true,
    };
    let summary = repository
        .summarize_usage_provider_performance(&query)
        .await
        .expect("provider performance should summarize");

    assert_eq!(summary.summary.request_count, 4);
    assert_eq!(summary.summary.success_count, 3);
    assert!((summary.summary.avg_output_tps.expect("summary tps") - 19.117_647).abs() < 0.001);
    assert_eq!(summary.summary.avg_first_byte_time_ms, Some(150.0));
    assert!(
        (summary
            .summary
            .avg_response_time_ms
            .expect("summary response")
            - 2333.333)
            .abs()
            < 0.001
    );

    assert_eq!(summary.providers.len(), 1);
    let provider = &summary.providers[0];
    assert_eq!(provider.provider_id, "provider-1");
    assert_eq!(provider.request_count, 3);
    assert_eq!(provider.success_count, 2);
    assert_eq!(provider.output_tokens, 1099);
    assert!((provider.avg_output_tps.expect("provider tps") - 26.315_789).abs() < 0.001);
    assert_eq!(provider.avg_first_byte_time_ms, Some(150.0));
    assert_eq!(provider.avg_response_time_ms, Some(2000.0));
    assert_eq!(provider.p90_response_time_ms, None);
    assert_eq!(provider.tps_sample_count, 2);
    assert_eq!(provider.first_byte_sample_count, 2);

    assert_eq!(summary.timeline.len(), 1);
    assert_eq!(summary.timeline[0].date, "2024-03-21T05:00:00+00:00");
    assert_eq!(summary.timeline[0].provider_id, "provider-1");
    assert!((summary.timeline[0].avg_output_tps.expect("timeline tps") - 26.315_789).abs() < 0.001);

    let mut without_timeline_query = query;
    without_timeline_query.include_timeline = false;
    let without_timeline = repository
        .summarize_usage_provider_performance(&without_timeline_query)
        .await
        .expect("provider performance without timeline should summarize");
    assert_eq!(without_timeline.summary, summary.summary);
    assert_eq!(without_timeline.providers, summary.providers);
    assert!(without_timeline.timeline.is_empty());
}

#[tokio::test]
async fn future_dashboard_summary_excludes_old_rows_and_preserves_canonical_cache_samples() {
    use aether_data_contracts::repository::usage::UsageDashboardAnalyticsQuery;
    let now = chrono::Utc::now();
    let since = now - chrono::Duration::minutes(5);
    let mut included = sample_usage(
        "future-summary",
        (now - chrono::Duration::seconds(1)).timestamp(),
    );
    included.api_key_id = None;
    included.api_format = Some("claude:messages".into());
    included.endpoint_api_format = Some("claude:messages".into());
    included.input_tokens = 80;
    included.output_tokens = 20;
    included.total_tokens = 150;
    included.cache_read_input_tokens = 40;
    included.cache_creation_input_tokens = 10;
    included.response_time_ms = Some(800);
    included.first_byte_time_ms = Some(100);
    included.is_stream = false;
    included.request_metadata = Some(json!({"upstream_is_stream": true}));
    included.actual_total_cost_usd = 0.12345678;
    included.billing_status = "settled".into();
    let mut excluded = included.clone();
    excluded.request_id = "before-activation".into();
    excluded.created_at_unix_ms = (since - chrono::Duration::days(500)).timestamp() as u64;
    let repo =
        InMemoryUsageReadRepository::seed([included, excluded]).with_dashboard_stats_since(since);
    let first = repo
        .query_dashboard_summary(&UsageDashboardAnalyticsQuery {
            timezone: "Asia/Kathmandu".into(),
        })
        .await
        .unwrap();
    assert_eq!(first.total.request_count, 1);
    assert_eq!(first.total.total_tokens, 150);
    assert_eq!(first.total.billable_amount.as_deref(), Some("0.12345678"));
    assert_eq!(first.today.cache_input_tokens, 130);
    assert_eq!(first.today.cache_read_tokens, 40);
    assert_eq!(first.today.first_byte_sample_count, 1);
    assert_eq!(first.today.response_sample_count, 1);
    assert_eq!(first.today.stream_requests, 1);
    assert_eq!(first.today.standard_requests, 0);
    assert_eq!(first.active_days, 1);
    assert_eq!(first.consecutive_active_days, 1);
    assert_eq!(
        first.activity_days.iter().map(|d| d.requests).sum::<u64>(),
        1
    );
    // Audit retention does not own the additive projection.
    repo.by_request_id.write().unwrap().clear();
    let retained = repo
        .query_dashboard_summary(&UsageDashboardAnalyticsQuery {
            timezone: "Asia/Kathmandu".into(),
        })
        .await
        .unwrap();
    assert_eq!(retained.total, first.total);
    assert_eq!(retained.stats_since, since.to_rfc3339());
}

#[tokio::test]
async fn future_dashboard_summary_streak_uses_local_days_before_heatmap_truncation() {
    use aether_data_contracts::repository::usage::UsageDashboardAnalyticsQuery;
    use chrono::TimeZone;

    let now = chrono::Utc::now();
    let tz = chrono_tz::Asia::Kathmandu;
    let today = now.with_timezone(&tz).date_naive();
    let rows = (1..=400).map(|offset| {
        let day = today - chrono::Duration::days(offset);
        let at = tz
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap();
        sample_usage(&format!("streak-{offset}"), at.timestamp())
    });
    let repo = InMemoryUsageReadRepository::seed(rows)
        .with_dashboard_stats_since(now - chrono::Duration::days(401));
    let summary = repo
        .query_dashboard_summary(&UsageDashboardAnalyticsQuery {
            timezone: tz.to_string(),
        })
        .await
        .unwrap();

    assert_eq!(summary.today.request_count, 0);
    assert_eq!(summary.active_days, 400);
    assert_eq!(summary.consecutive_active_days, 400);
    assert_eq!(summary.activity_days.len(), 364);
    assert_eq!(
        summary.activity_days.last().unwrap().date,
        today.pred_opt().unwrap().to_string()
    );
}

/// 回归测试：健康监控时间线条必须覆盖完整回溯窗口。
///
/// 历史缺陷：时间线条此前是用"最近 N 条事件样本"在内存里分桶，
/// 于是 6 小时内请求数超过 N 的模型，早期时段会被判成"无请求"（灰条），
/// 与卡片头部的全窗口请求数自相矛盾。这里验证聚合直接按全窗口分桶，不受样本条数影响。
#[tokio::test]
async fn usage_health_timeline_covers_full_window_regardless_of_sample_size() {
    use aether_data_contracts::repository::usage::{
        UsageHealthTimelineGroupBy, UsageHealthTimelineQuery,
    };

    let segments = 60u32;
    let window_secs = 6 * 3600u64; // 6 小时回溯窗口
    let now = 1_800_000_000u64;
    let since = now - window_secs;
    // 265 次请求均匀铺满整个窗口，远多于旧的默认样本上限（60/100 条）。
    let rows = (0..265u64)
        .map(|index| {
            let at = since + index * window_secs / 265;
            let mut usage = sample_usage(&format!("health-timeline-{index}"), at as i64);
            usage.model = "deepseek-v4.1-flash".to_string();
            usage.provider_name = "codebuddy2api".to_string();
            usage
        })
        .collect::<Vec<_>>();
    let repo = InMemoryUsageReadRepository::seed(rows);

    let buckets = repo
        .aggregate_usage_health_timeline(&UsageHealthTimelineQuery {
            created_from_unix_secs: since,
            created_until_unix_secs: now,
            group_by: UsageHealthTimelineGroupBy::Model,
            group_values: vec!["deepseek-v4.1-flash".to_string()],
            provider_name: None,
            model: None,
            api_format: None,
            segments,
            exclude_status_codes: vec![499],
        })
        .await
        .unwrap();

    // 全窗口分桶：几乎每一段都应有样本，总请求数必须等于窗口内的全部请求。
    assert!(
        buckets.len() >= (segments as usize) - 1,
        "时间线应覆盖整个窗口，实际只有 {} 段有数据",
        buckets.len()
    );
    let total_requests: u64 = buckets.iter().map(|row| row.request_count).sum();
    assert_eq!(total_requests, 265);
    assert!(buckets.iter().all(|row| row.segment_idx < segments));
    assert!(buckets
        .iter()
        .all(|row| row.success_count == row.request_count));

    // 按提供商维度聚合时，同一批请求应落在同一个分组键下。
    let provider_buckets = repo
        .aggregate_usage_health_timeline(&UsageHealthTimelineQuery {
            created_from_unix_secs: since,
            created_until_unix_secs: now,
            group_by: UsageHealthTimelineGroupBy::Provider,
            group_values: vec!["codebuddy2api".to_string()],
            provider_name: None,
            model: None,
            api_format: None,
            segments,
            exclude_status_codes: vec![499],
        })
        .await
        .unwrap();
    let provider_total: u64 = provider_buckets.iter().map(|row| row.request_count).sum();
    assert_eq!(
        provider_total, total_requests,
        "唯一提供商下的模型与提供商自身必须看到同一批请求"
    );
}
