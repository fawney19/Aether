use super::{SqliteUsageReadRepository, SqliteUsageWriteRepository};
use crate::run_migrations;
use aether_data_contracts::repository::usage::{
    ProviderApiKeyWindowUsageRequest, UpsertUsageRecord, UsageAuditAggregationGroupBy,
    UsageAuditAggregationQuery, UsageAuditListQuery, UsageAuditSummaryQuery, UsageBodyCaptureState,
    UsageBreakdownGroupBy, UsageBreakdownSummaryQuery, UsageCleanupExecutionMode,
    UsageCleanupTargets, UsageCleanupWindow, UsageDailyActualCostRollupQuery,
    UsageDailyHeatmapQuery, UsageDashboardDailyBreakdownQuery, UsageDashboardSummaryQuery,
    UsageProviderPerformanceQuery, UsageReadRepository, UsageTimeSeriesGranularity,
    UsageWriteRepository,
};
use chrono::{DateTime, Utc};

#[test]
fn sqlite_usage_upsert_guards_candidate_identity_metadata_and_routing_from_late_lifecycle() {
    for field in [
        "provider_name",
        "model",
        "target_model",
        "provider_id",
        "provider_endpoint_id",
        "provider_api_key_id",
        "request_type",
        "api_format",
        "api_family",
        "endpoint_kind",
        "endpoint_api_format",
        "provider_api_family",
        "provider_endpoint_kind",
        "has_format_conversion",
        "is_stream",
        "upstream_is_stream",
        "request_metadata",
        "candidate_id",
        "candidate_index",
        "key_name",
        "planner_kind",
        "route_family",
        "route_kind",
        "execution_path",
        "local_execution_runtime_miss_reason",
    ] {
        let assignment = format!("{field} = CASE WHEN (");
        assert!(
            super::UPSERT_USAGE_SQL.contains(&assignment),
            "missing lifecycle guard for {field}"
        );
        assert!(
            super::UPSERT_USAGE_SQL.contains(&format!("THEN \"usage\".{field}")),
            "late lifecycle must preserve {field}"
        );
    }
    assert!(super::UPSERT_USAGE_SQL
        .contains("OR (\"usage\".status = 'streaming' AND excluded.status = 'pending')"));
}

#[tokio::test]
async fn sqlite_provider_performance_can_skip_timeline() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    SqliteUsageWriteRepository::new(pool.clone())
        .upsert(sample_usage(
            "provider-performance",
            "completed",
            "pending",
            1_000,
        ))
        .await
        .expect("usage should upsert");
    let reader = SqliteUsageReadRepository::new(pool);
    let mut query = UsageProviderPerformanceQuery {
        created_from_unix_secs: 0,
        created_until_unix_secs: 2_000,
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

    let with_timeline = reader
        .summarize_usage_provider_performance(&query)
        .await
        .expect("provider performance should load");
    assert_eq!(with_timeline.summary.request_count, 1);
    assert_eq!(with_timeline.providers.len(), 1);
    assert_eq!(with_timeline.timeline.len(), 1);

    query.include_timeline = false;
    let without_timeline = reader
        .summarize_usage_provider_performance(&query)
        .await
        .expect("provider performance without timeline should load");
    assert_eq!(without_timeline.summary, with_timeline.summary);
    assert_eq!(without_timeline.providers, with_timeline.providers);
    assert!(without_timeline.timeline.is_empty());
}

#[tokio::test]
async fn sqlite_daily_actual_cost_rollups_group_normal_and_standalone_keys() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let writer = SqliteUsageWriteRepository::new(pool.clone());
    writer
        .upsert(sample_usage("daily-normal", "completed", "settled", 100))
        .await
        .expect("normal daily usage should seed");
    let mut standalone = sample_usage("daily-standalone", "completed", "settled", 100);
    standalone.api_key_id = Some("standalone-key".to_string());
    standalone.actual_total_cost_usd = Some(0.6);
    standalone.request_metadata = Some(serde_json::json!({
        "api_key_is_standalone": true,
    }));
    writer
        .upsert(standalone)
        .await
        .expect("standalone daily usage should seed");

    let rollups = SqliteUsageReadRepository::new(pool)
        .summarize_usage_daily_actual_cost_rollups(&UsageDailyActualCostRollupQuery {
            finalized_from_unix_secs: 50,
            finalized_until_unix_secs: 150,
        })
        .await
        .expect("daily usage rollups should load");
    assert_eq!(rollups.len(), 2);
    assert!(rollups.iter().any(|rollup| {
        rollup.api_key_id == "api-key-1"
            && !rollup.api_key_is_standalone
            && rollup.actual_total_cost_usd == 0.4
    }));
    assert!(rollups.iter().any(|rollup| {
        rollup.api_key_id == "standalone-key"
            && rollup.api_key_is_standalone
            && rollup.actual_total_cost_usd == 0.6
    }));
}

#[tokio::test]
async fn sqlite_usage_write_repository_upserts_and_flushes_counter_deltas() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool.clone());
    let record = repository
        .upsert(sample_usage("request-1", "completed", "pending", 1_000))
        .await
        .expect("usage should upsert");

    assert_eq!(record.request_id, "request-1");
    assert_eq!(record.api_key_id.as_deref(), Some("api-key-1"));
    assert_eq!(record.total_tokens, 5);
    assert_eq!(record.cache_read_input_tokens, 2);
    assert_eq!(
        record.request_metadata.as_ref().unwrap()["trace_id"],
        "trace-1"
    );
    assert_eq!(
        record.request_metadata.as_ref().unwrap()["upstream_is_stream"],
        true
    );
    let upstream_is_stream: Option<i64> =
        sqlx::query_scalar("SELECT upstream_is_stream FROM \"usage\" WHERE request_id = ?")
            .bind("request-1")
            .fetch_one(&pool)
            .await
            .expect("usage stream mode should load");
    assert_eq!(upstream_is_stream, Some(1));

    repository
        .flush_usage_counter_deltas(100)
        .await
        .expect("usage counter deltas should flush");

    let loaded = repository
        .find_by_request_id("request-1")
        .await
        .expect("usage should load")
        .expect("usage should exist");
    assert_eq!(
        loaded.provider_api_key_id.as_deref(),
        Some("provider-key-1")
    );

    let stats = sqlx::query_as::<_, (i64, i64, f64, Option<i64>)>(
            "SELECT total_requests, total_tokens, total_cost_usd, last_used_at FROM api_keys WHERE id = 'api-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("api key stats should load");
    assert_eq!(stats, (1, 5, 0.5, Some(1_000)));

    let provider_stats = sqlx::query_as::<_, (i64, i64, i64, i64, f64, i64, Option<i64>)>(
            "SELECT request_count, success_count, error_count, total_tokens, total_cost_usd, total_response_time_ms, last_used_at FROM provider_api_keys WHERE id = 'provider-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("provider key stats should load");
    assert_eq!(provider_stats, (1, 1, 0, 5, 0.5, 42, Some(1_000)));
    let model_usage_count: i64 =
        sqlx::query_scalar("SELECT usage_count FROM global_models WHERE name = 'model-1'")
            .fetch_one(&pool)
            .await
            .expect("global model usage count should load");
    assert_eq!(model_usage_count, 1);

    repository
        .upsert(sample_usage("request-1", "completed", "pending", 1_000))
        .await
        .expect("identical terminal usage should remain idempotent");
    repository
        .flush_usage_counter_deltas(100)
        .await
        .expect("idempotent counter flush should succeed");
    let repeated_stats = sqlx::query_as::<_, (i64, i64, f64)>(
        "SELECT total_requests, total_tokens, total_cost_usd FROM api_keys WHERE id = 'api-key-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("repeated api key stats should load");
    assert_eq!(repeated_stats, (1, 5, 0.5));
    let repeated_provider_requests: i64 = sqlx::query_scalar(
        "SELECT request_count FROM provider_api_keys WHERE id = 'provider-key-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("repeated provider stats should load");
    assert_eq!(repeated_provider_requests, 1);
    let repeated_model_usage_count: i64 =
        sqlx::query_scalar("SELECT usage_count FROM global_models WHERE name = 'model-1'")
            .fetch_one(&pool)
            .await
            .expect("repeated global model usage count should load");
    assert_eq!(repeated_model_usage_count, 1);
}

#[tokio::test]
async fn sqlite_usage_stats_rebuild_uses_canonical_terminal_totals() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let repository = SqliteUsageWriteRepository::new(pool.clone());

    repository
        .upsert(sample_usage(
            "rebuild-completed",
            "completed",
            "pending",
            1_000,
        ))
        .await
        .expect("completed usage should upsert");
    repository
        .upsert(sample_usage("rebuild-pending", "pending", "pending", 2_000))
        .await
        .expect("pending usage should upsert");
    sqlx::query(
        r#"
UPDATE "usage"
SET total_tokens = 99
WHERE request_id = 'rebuild-completed';
UPDATE usage_settlement_snapshots
SET billing_effective_input_tokens = 11,
    billing_output_tokens = 13,
    billing_cache_creation_tokens = 2,
    billing_cache_read_tokens = 3,
    billing_total_input_context = NULL
WHERE request_id = 'rebuild-completed';
"#,
    )
    .execute(&pool)
    .await
    .expect("conflicting raw and settlement token totals should seed");

    let rebuilt = repository
        .rebuild_api_key_usage_stats()
        .await
        .expect("api key stats should rebuild");
    let stats: (i64, i64, f64, Option<i64>) = sqlx::query_as(
        "SELECT total_requests, total_tokens, total_cost_usd, last_used_at FROM api_keys WHERE id = 'api-key-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("rebuilt api key stats should load");

    assert_eq!(rebuilt, 1);
    assert_eq!(stats, (1, 29, 0.5, Some(1_000)));

    let provider_rebuilt = repository
        .rebuild_provider_api_key_usage_stats()
        .await
        .expect("provider api key stats should rebuild");
    let provider_stats: (i64, i64, i64, i64, f64, Option<i64>) = sqlx::query_as(
        "SELECT request_count, success_count, error_count, total_tokens, total_cost_usd, last_used_at FROM provider_api_keys WHERE id = 'provider-key-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("rebuilt provider api key stats should load");

    assert_eq!(provider_rebuilt, 1);
    assert_eq!(provider_stats, (2, 1, 0, 29, 0.5, Some(2_000)));

    let reader = SqliteUsageReadRepository::new(pool);
    let user_totals = reader
        .summarize_usage_totals_by_user_ids(&["user-1".to_string()])
        .await
        .expect("user totals should load");
    assert_eq!(user_totals[0].request_count, 1);
    assert_eq!(user_totals[0].total_tokens, 29);

    let api_key_totals = reader
        .summarize_total_tokens_by_api_key_ids(&["api-key-1".to_string()])
        .await
        .expect("api key totals should load");
    assert_eq!(api_key_totals["api-key-1"], 34);

    let provider_key_totals = reader
        .summarize_usage_by_provider_api_key_ids(&["provider-key-1".to_string()])
        .await
        .expect("provider key totals should load");
    assert_eq!(provider_key_totals["provider-key-1"].total_tokens, 34);

    let provider_window = reader
        .summarize_usage_by_provider_api_key_windows(&[ProviderApiKeyWindowUsageRequest {
            provider_api_key_id: "provider-key-1".to_string(),
            window_code: "test".to_string(),
            start_unix_secs: 0,
            end_unix_secs: 3_000,
        }])
        .await
        .expect("provider key window should load");
    assert_eq!(provider_window[0].total_tokens, 34);

    let audit_summary = reader
        .summarize_usage_audits(&UsageAuditSummaryQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 3_000,
            ..UsageAuditSummaryQuery::default()
        })
        .await
        .expect("usage audit summary should load");
    assert_eq!(audit_summary.recorded_total_tokens, 34);

    let aggregation = reader
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 3_000,
            group_by: UsageAuditAggregationGroupBy::Model,
            limit: 10,
            exclude_reserved_provider_labels: false,
        })
        .await
        .expect("usage audit aggregation should load");
    assert_eq!(aggregation[0].total_tokens, 29);

    let breakdown = reader
        .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 3_000,
            group_by: UsageBreakdownGroupBy::Model,
            ..UsageBreakdownSummaryQuery::default()
        })
        .await
        .expect("usage breakdown should load");
    assert_eq!(breakdown[0].total_tokens, 29);

    let daily = reader
        .list_dashboard_daily_breakdown(&UsageDashboardDailyBreakdownQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 3_000,
            tz_offset_minutes: 0,
            user_id: Some("user-1".to_string()),
        })
        .await
        .expect("dashboard daily breakdown should load");
    assert_eq!(daily[0].total_tokens, 29);
}

#[tokio::test]
async fn sqlite_usage_http_capture_round_trips_and_preserves_sparse_updates() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let writer = SqliteUsageWriteRepository::new(pool.clone());

    let mut rich = sample_usage("canonical-capture", "pending", "pending", 1_000);
    rich.request_headers = Some(serde_json::json!({"x-client": "one"}));
    rich.provider_request_headers = Some(serde_json::json!({"x-provider": "two"}));
    rich.response_headers = Some(serde_json::json!({"x-upstream": "three"}));
    rich.client_response_headers = Some(serde_json::json!({"x-response": "four"}));
    rich.request_body = Some(serde_json::json!({"request": 1}));
    rich.provider_request_body = Some(serde_json::json!({"provider_request": 2}));
    rich.response_body = Some(serde_json::json!({"response": 3}));
    rich.client_response_body = Some(serde_json::json!({"client_response": 4}));
    rich.request_body_state = Some(UsageBodyCaptureState::Inline);
    rich.provider_request_body_state = Some(UsageBodyCaptureState::Inline);
    rich.response_body_state = Some(UsageBodyCaptureState::Inline);
    rich.client_response_body_state = Some(UsageBodyCaptureState::Inline);
    rich.request_metadata = Some(serde_json::json!({
        "trace_id": "canonical-trace",
        "request_body_ref": "usage://request/stale/request_body"
    }));

    let stored = writer
        .upsert(rich)
        .await
        .expect("canonical capture should upsert");
    assert_eq!(
        stored.request_headers,
        Some(serde_json::json!({"x-client": "one"}))
    );
    assert_eq!(stored.request_body, Some(serde_json::json!({"request": 1})));
    assert_eq!(
        stored.provider_request_body,
        Some(serde_json::json!({"provider_request": 2}))
    );
    assert_eq!(
        stored.response_body,
        Some(serde_json::json!({"response": 3}))
    );
    assert_eq!(
        stored.client_response_body,
        Some(serde_json::json!({"client_response": 4}))
    );
    assert_eq!(
        stored.request_body_state,
        Some(UsageBodyCaptureState::Reference)
    );
    assert_eq!(
        stored.request_body_ref.as_deref(),
        Some("usage://request/canonical-capture/request_body")
    );
    assert_eq!(
        stored.request_metadata.as_ref().unwrap()["trace_id"],
        "canonical-trace"
    );
    assert!(stored
        .request_metadata
        .as_ref()
        .unwrap()
        .get("request_body_ref")
        .is_none());

    let legacy_columns: (Option<String>, Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT request_headers, request_body, request_body_compressed FROM \"usage\" WHERE request_id = 'canonical-capture'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy columns should load");
    assert_eq!(legacy_columns, (None, None, None));
    let audit: (String, String, String) = sqlx::query_as(
        "SELECT request_headers, request_body_ref, request_body_state FROM usage_http_audits WHERE request_id = 'canonical-capture'",
    )
    .fetch_one(&pool)
    .await
    .expect("canonical audit should load");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&audit.0).expect("header JSON should decode"),
        serde_json::json!({"x-client": "one"})
    );
    assert_eq!(audit.1, "usage://request/canonical-capture/request_body");
    assert_eq!(audit.2, "reference");
    let blob_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_body_blobs WHERE request_id = 'canonical-capture'",
    )
    .fetch_one(&pool)
    .await
    .expect("canonical blobs should count");
    assert_eq!(blob_count, 4);

    let sparse = sample_usage("canonical-capture", "streaming", "pending", 1_001);
    let sparse_stored = writer
        .upsert(sparse)
        .await
        .expect("sparse lifecycle update should upsert");
    assert_eq!(sparse_stored.request_headers, stored.request_headers);
    assert_eq!(sparse_stored.request_body, stored.request_body);
    assert_eq!(sparse_stored.response_body, stored.response_body);
    let sparse_blob_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_body_blobs WHERE request_id = 'canonical-capture'",
    )
    .fetch_one(&pool)
    .await
    .expect("preserved blobs should count");
    assert_eq!(sparse_blob_count, 4);

    let mut clear = sample_usage("canonical-capture", "streaming", "pending", 1_002);
    clear.request_body = Some(serde_json::json!({"residual": true}));
    clear.request_body_ref = Some("usage://request/canonical-capture/request_body".to_string());
    clear.request_body_state = Some(UsageBodyCaptureState::None);
    let cleared = writer
        .upsert(clear)
        .await
        .expect("explicit none capture should clear");
    assert!(cleared.request_body.is_none());
    assert!(cleared.request_body_ref.is_none());
    assert_eq!(
        cleared.request_body_state,
        Some(UsageBodyCaptureState::None)
    );
    assert_eq!(cleared.provider_request_body, stored.provider_request_body);
    let cleared_blob_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_body_blobs WHERE request_id = 'canonical-capture'",
    )
    .fetch_one(&pool)
    .await
    .expect("remaining blobs should count");
    assert_eq!(cleared_blob_count, 3);

    let reader = SqliteUsageReadRepository::new(pool.clone());
    let resolved = reader
        .resolve_body_ref("usage://request/canonical-capture/provider_request_body")
        .await
        .expect("body ref should resolve");
    assert_eq!(resolved, stored.provider_request_body);
    let loaded = reader
        .find_by_request_id("canonical-capture")
        .await
        .expect("canonical usage should load")
        .expect("canonical usage should exist");
    assert_eq!(
        loaded.provider_request_headers,
        stored.provider_request_headers
    );
    assert_eq!(loaded.provider_request_body, stored.provider_request_body);
}

#[tokio::test]
async fn sqlite_usage_http_read_falls_back_to_legacy_inline_and_compressed_columns() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let writer = SqliteUsageWriteRepository::new(pool.clone());
    let mut captured = sample_usage("legacy-capture", "pending", "pending", 2_000);
    captured.request_body = Some(serde_json::json!({"compressed": true}));
    writer
        .upsert(captured)
        .await
        .expect("temporary canonical body should upsert");
    let payload: Vec<u8> = sqlx::query_scalar(
        "SELECT payload_gzip FROM usage_body_blobs WHERE request_id = 'legacy-capture' AND body_field = 'request_body'",
    )
    .fetch_one(&pool)
    .await
    .expect("temporary gzip should load");
    sqlx::query(
        r#"
DELETE FROM usage_http_audits WHERE request_id = 'legacy-capture';
DELETE FROM usage_body_blobs WHERE request_id = 'legacy-capture';
UPDATE "usage"
SET request_headers = '{"legacy":true}',
    request_body_compressed = ?,
    response_body = '{"inline":true}',
    request_metadata = '{"request_body_ref":"usage://request/legacy-capture/request_body"}'
WHERE request_id = 'legacy-capture';
"#,
    )
    .bind(payload)
    .execute(&pool)
    .await
    .expect("legacy capture should seed");

    let reader = SqliteUsageReadRepository::new(pool.clone());
    let loaded = reader
        .find_by_request_id("legacy-capture")
        .await
        .expect("legacy usage should load")
        .expect("legacy usage should exist");
    assert_eq!(
        loaded.request_headers,
        Some(serde_json::json!({"legacy": true}))
    );
    assert_eq!(
        loaded.request_body,
        Some(serde_json::json!({"compressed": true}))
    );
    assert_eq!(
        loaded.response_body,
        Some(serde_json::json!({"inline": true}))
    );
    assert_eq!(
        loaded.request_body_ref.as_deref(),
        Some("usage://request/legacy-capture/request_body")
    );
    assert!(loaded.request_body_state.is_none());

    let mut clear = sample_usage("legacy-capture", "streaming", "pending", 2_001);
    clear.request_metadata = None;
    clear.request_body_state = Some(UsageBodyCaptureState::None);
    let cleared = writer
        .upsert(clear)
        .await
        .expect("explicit none should clear legacy fallback storage");
    assert!(cleared.request_body.is_none());
    assert!(cleared.request_body_ref.is_none());
    assert_eq!(
        cleared.request_body_state,
        Some(UsageBodyCaptureState::None)
    );
    assert!(cleared
        .request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .is_some_and(|metadata| !metadata.contains_key("request_body_ref")));
    let compressed_after_clear: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT request_body_compressed FROM \"usage\" WHERE request_id = 'legacy-capture'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy compressed body should load after clear");
    assert!(compressed_after_clear.is_none());
}

#[tokio::test]
async fn sqlite_usage_canonical_snapshots_round_trip_preserve_sparse_and_clear_terminal() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let writer = SqliteUsageWriteRepository::new(pool.clone());
    let reader = SqliteUsageReadRepository::new(pool.clone());

    let mut rich = sample_usage("canonical-snapshots", "streaming", "pending", 1_000);
    rich.candidate_id = None;
    rich.candidate_index = None;
    rich.key_name = None;
    rich.planner_kind = None;
    rich.route_family = None;
    rich.route_kind = None;
    rich.execution_path = None;
    rich.input_tokens = Some(1_000);
    rich.output_tokens = Some(100);
    rich.cache_creation_input_tokens = Some(0);
    rich.cache_creation_ephemeral_5m_input_tokens = Some(100);
    rich.cache_creation_ephemeral_1h_input_tokens = Some(50);
    rich.cache_read_input_tokens = Some(200);
    rich.request_metadata = Some(serde_json::json!({
        "trace_id": "rich",
        "candidate_id": "candidate-canonical",
        "candidate_index": 4,
        "key_name": "key-canonical",
        "planner_kind": "fallback",
        "route_family": "chat",
        "route_kind": "remote",
        "execution_path": "converted",
        "local_execution_runtime_miss_reason": "runtime_busy",
        "billing_snapshot_schema_version": "v3",
        "billing_snapshot_status": "resolved",
        "rate_multiplier": 0.75,
        "is_free_tier": false,
        "input_price_per_1m": 1.1,
        "output_price_per_1m": 2.2,
        "cache_creation_price_per_1m": 3.3,
        "cache_read_price_per_1m": 4.4,
        "price_per_request": 0.05,
        "billing_dimensions": {
            "input_tokens": 1000,
            "effective_input_tokens": 650,
            "output_tokens": 100,
            "cache_creation_tokens": 150,
            "cache_creation_ephemeral_5m_tokens": 100,
            "cache_creation_ephemeral_1h_tokens": 50,
            "cache_read_tokens": 200,
            "total_input_context": 1000
        },
        "settlement_snapshot": {
            "schema_version": "v3",
            "total_cost": 1.25,
            "actual_total_cost": 1.0,
            "cost_breakdown": {
                "cache_creation_ephemeral_5m_cost": 0.02,
                "cache_creation_ephemeral_1h_cost": 0.03,
                "cache_read_cost": 0.04
            },
            "pricing_snapshot": {"pricing_source": "catalog"},
            "billing_plan_snapshot": {"rule_id": "rule-1", "rule_version": "7"}
        }
    }));
    let stored = writer
        .upsert(rich)
        .await
        .expect("canonical snapshots should upsert");
    assert_eq!(stored.routing_candidate_id(), Some("candidate-canonical"));
    assert_eq!(stored.routing_candidate_index(), Some(4));
    assert_eq!(stored.provider_id.as_deref(), Some("provider-1"));
    assert_eq!(stored.output_tokens, 100);
    assert_eq!(stored.cache_creation_input_tokens, 150);
    assert_eq!(stored.cache_read_input_tokens, 200);
    assert_eq!(stored.total_tokens, 1_100);
    assert_eq!(stored.total_cost_usd, 1.25);
    assert_eq!(stored.actual_total_cost_usd, 1.0);
    assert_eq!(stored.cache_creation_cost_usd, 0.05);
    assert_eq!(stored.cache_read_cost_usd, 0.04);
    assert_eq!(stored.settlement_rate_multiplier(), Some(0.75));
    assert_eq!(stored.settlement_input_price_per_1m(), Some(1.1));
    assert_eq!(stored.settlement_output_price_per_1m(), Some(2.2));
    assert_eq!(stored.settlement_price_per_request(), Some(0.05));

    sqlx::query(
        r#"
UPDATE usage_settlement_snapshots
SET wallet_id = 'wallet-sentinel',
    wallet_balance_before = 10,
    wallet_balance_after = 9,
    provider_monthly_used_usd = 8,
    finalized_at = 2000
WHERE request_id = 'canonical-snapshots'
"#,
    )
    .execute(&pool)
    .await
    .expect("wallet settlement facts should seed");

    let mut sparse = sample_usage("canonical-snapshots", "streaming", "pending", 1_001);
    sparse.provider_id = None;
    sparse.provider_endpoint_id = None;
    sparse.provider_api_key_id = None;
    sparse.has_format_conversion = None;
    sparse.candidate_id = None;
    sparse.candidate_index = None;
    sparse.key_name = None;
    sparse.planner_kind = None;
    sparse.route_family = None;
    sparse.route_kind = None;
    sparse.execution_path = None;
    sparse.local_execution_runtime_miss_reason = None;
    sparse.input_tokens = None;
    sparse.output_tokens = None;
    sparse.total_tokens = None;
    sparse.cache_creation_input_tokens = None;
    sparse.cache_creation_ephemeral_5m_input_tokens = None;
    sparse.cache_creation_ephemeral_1h_input_tokens = None;
    sparse.cache_read_input_tokens = None;
    sparse.cache_creation_cost_usd = None;
    sparse.cache_read_cost_usd = None;
    sparse.output_price_per_1m = None;
    sparse.total_cost_usd = None;
    sparse.actual_total_cost_usd = None;
    sparse.request_metadata = Some(serde_json::json!({"trace_id": "sparse"}));
    let sparse_stored = writer
        .upsert(sparse)
        .await
        .expect("sparse snapshots should merge");
    assert_eq!(
        sparse_stored.routing_candidate_id(),
        Some("candidate-canonical")
    );
    assert_eq!(sparse_stored.provider_id.as_deref(), Some("provider-1"));
    assert_eq!(sparse_stored.output_tokens, 100);
    assert_eq!(sparse_stored.total_tokens, 1_100);
    assert_eq!(sparse_stored.total_cost_usd, 1.25);
    assert_eq!(sparse_stored.trace_id(), Some("sparse"));
    assert_eq!(sparse_stored.settlement_rate_multiplier(), Some(0.75));

    sqlx::query(
        r#"
UPDATE "usage"
SET candidate_id = 'legacy-candidate',
    route_family = 'legacy-route',
    total_cost_usd = 99,
    output_price_per_1m = 99,
    request_metadata = '{"trace_id":"legacy","rate_multiplier":9}'
WHERE request_id = 'canonical-snapshots'
"#,
    )
    .execute(&pool)
    .await
    .expect("legacy mirrors should be corruptible for precedence test");
    let canonical = reader
        .find_by_request_id("canonical-snapshots")
        .await
        .expect("canonical usage should load")
        .expect("canonical usage should exist");
    assert_eq!(
        canonical.routing_candidate_id(),
        Some("candidate-canonical")
    );
    assert_eq!(canonical.routing_route_family(), Some("chat"));
    assert_eq!(canonical.total_cost_usd, 1.25);
    assert_eq!(canonical.settlement_output_price_per_1m(), Some(2.2));
    assert_eq!(canonical.settlement_rate_multiplier(), Some(0.75));

    let mut terminal = sample_usage("canonical-snapshots", "completed", "settled", 1_002);
    terminal.provider_id = None;
    terminal.provider_endpoint_id = None;
    terminal.provider_api_key_id = None;
    terminal.has_format_conversion = None;
    terminal.candidate_id = None;
    terminal.candidate_index = None;
    terminal.key_name = None;
    terminal.planner_kind = None;
    terminal.route_family = None;
    terminal.route_kind = None;
    terminal.execution_path = None;
    terminal.local_execution_runtime_miss_reason = None;
    terminal.input_tokens = None;
    terminal.output_tokens = None;
    terminal.total_tokens = None;
    terminal.cache_creation_input_tokens = None;
    terminal.cache_creation_ephemeral_5m_input_tokens = None;
    terminal.cache_creation_ephemeral_1h_input_tokens = None;
    terminal.cache_read_input_tokens = None;
    terminal.cache_creation_cost_usd = None;
    terminal.cache_read_cost_usd = None;
    terminal.output_price_per_1m = None;
    terminal.total_cost_usd = None;
    terminal.actual_total_cost_usd = None;
    terminal.request_metadata = Some(serde_json::json!({"trace_id": "terminal"}));
    let terminal_stored = writer
        .upsert(terminal)
        .await
        .expect("terminal snapshots should replace");
    assert_eq!(terminal_stored.status, "completed");
    assert_eq!(terminal_stored.billing_status, "settled");
    assert!(terminal_stored.candidate_id.is_none());
    assert!(terminal_stored.provider_id.is_none());
    assert_eq!(terminal_stored.total_tokens, 0);
    assert_eq!(terminal_stored.total_cost_usd, 0.0);
    assert_eq!(terminal_stored.settlement_rate_multiplier(), None);
    assert_eq!(terminal_stored.trace_id(), Some("terminal"));

    let terminal_row: (Option<String>, Option<String>, String, Option<f64>) = sqlx::query_as(
        r#"
SELECT routing.candidate_id, settlement.settlement_snapshot,
       settlement.billing_status, settlement.billing_total_cost_usd
FROM usage_routing_snapshots routing
JOIN usage_settlement_snapshots settlement USING (request_id)
WHERE routing.request_id = 'canonical-snapshots'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("terminal canonical rows should load");
    assert_eq!(terminal_row, (None, None, "settled".to_string(), None));
    let wallet_row = sqlx::query(
        r#"
SELECT wallet_id, wallet_balance_before, wallet_balance_after,
       provider_monthly_used_usd, finalized_at
FROM usage_settlement_snapshots
WHERE request_id = 'canonical-snapshots'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("wallet settlement facts should load");
    assert_eq!(
        sqlx::Row::try_get::<Option<String>, _>(&wallet_row, "wallet_id")
            .expect("wallet id should decode"),
        Some("wallet-sentinel".to_string())
    );
    assert_eq!(
        sqlx::Row::try_get::<Option<f64>, _>(&wallet_row, "wallet_balance_before")
            .expect("wallet balance before should decode"),
        Some(10.0)
    );
    assert_eq!(
        sqlx::Row::try_get::<Option<f64>, _>(&wallet_row, "wallet_balance_after")
            .expect("wallet balance after should decode"),
        Some(9.0)
    );
    assert_eq!(
        sqlx::Row::try_get::<Option<f64>, _>(&wallet_row, "provider_monthly_used_usd")
            .expect("provider monthly usage should decode"),
        Some(8.0)
    );
    assert_eq!(
        sqlx::Row::try_get::<Option<i64>, _>(&wallet_row, "finalized_at")
            .expect("settlement finalized_at should decode"),
        Some(2_000)
    );
    assert_eq!(terminal_stored.finalized_at_unix_secs, Some(2_000));

    let mut late = sample_usage("canonical-snapshots", "pending", "pending", 1_003);
    late.candidate_id = Some("late-candidate".to_string());
    late.request_metadata = Some(serde_json::json!({
        "trace_id": "late",
        "rate_multiplier": 9,
        "settlement_snapshot": {"schema_version": "late", "total_cost": 99}
    }));
    let after_late = writer
        .upsert(late)
        .await
        .expect("late pending usage should return terminal record");
    assert_eq!(after_late.status, "completed");
    assert_eq!(after_late.billing_status, "settled");
    assert!(after_late.candidate_id.is_none());
    assert_eq!(after_late.total_cost_usd, 0.0);
    assert_eq!(after_late.trace_id(), Some("terminal"));
}

#[tokio::test]
async fn sqlite_usage_cleanup_matches_policy_windows_and_targets() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let repository = SqliteUsageWriteRepository::new(pool.clone());

    for (request_id, created_at) in [
        ("cleanup-log", 10),
        ("cleanup-stale-body", 30),
        ("cleanup-header", 50),
        ("cleanup-detail", 70),
        ("cleanup-legacy", 75),
        ("cleanup-new", 90),
    ] {
        repository
            .upsert(sample_usage(request_id, "completed", "settled", created_at))
            .await
            .expect("usage should seed");
    }

    sqlx::query(
        r#"
UPDATE "usage"
SET request_headers = '{"old":true}', request_body = '{"delete":true}'
WHERE request_id = 'cleanup-log';
UPDATE "usage"
SET request_headers = '{"stale":true}', request_body_compressed = X'1F8B'
WHERE request_id = 'cleanup-stale-body';
UPDATE "usage"
SET response_headers = '{"header":true}'
WHERE request_id = 'cleanup-header';
UPDATE "usage"
SET request_body = '{"detail":true}'
WHERE request_id = 'cleanup-detail';
UPDATE "usage"
SET request_metadata = '{"trace":"kept","request_body_ref":"usage://request/cleanup-legacy/request_body"}'
WHERE request_id = 'cleanup-legacy';
UPDATE "usage"
SET request_headers = '{"new":true}', request_body = '{"new":true}'
WHERE request_id = 'cleanup-new';

INSERT INTO usage_body_blobs (body_ref, request_id, body_field, payload_gzip)
VALUES ('usage-body://cleanup-stale-body/request_body', 'cleanup-stale-body', 'request_body', X'1F8B');
INSERT INTO usage_http_audits (
  request_id, request_headers, request_body_ref, body_capture_mode
)
VALUES (
  'cleanup-stale-body', '{"audit":true}',
  'usage-body://cleanup-stale-body/request_body', 'ref_backed'
);

INSERT INTO api_keys (
  id, user_id, key_hash, is_active, auto_delete_on_expiry,
  expires_at, created_at, updated_at
)
VALUES
  ('cleanup-disable-key', 'user-1', 'cleanup-disable-hash', 1, 0, 1, 1, 1),
  ('cleanup-delete-key', 'user-1', 'cleanup-delete-hash', 1, 1, 1, 1, 1);
INSERT INTO wallets (
  id, api_key_id, balance, gift_balance, limit_mode, currency, status,
  created_at, updated_at
)
VALUES (
  'cleanup-delete-wallet', 'cleanup-delete-key', 0, 0, 'finite', 'USD', 'active', 1, 1
);
"#,
    )
    .execute(&pool)
    .await
    .expect("cleanup fixtures should seed");

    let window = cleanup_window(80, 60, 70, 20);
    let preview = repository
        .preview_usage_cleanup(
            &window,
            UsageCleanupTargets::all_policy_targets(),
            UsageCleanupExecutionMode::Policy,
        )
        .await
        .expect("cleanup preview should load");
    assert_eq!(preview.detail, 2);
    assert_eq!(preview.compressed, 1);
    assert_eq!(preview.header, 2);
    assert_eq!(preview.log, 1);

    let summary = repository
        .cleanup_usage(
            &window,
            1,
            true,
            UsageCleanupTargets::all_policy_targets(),
            UsageCleanupExecutionMode::Policy,
        )
        .await
        .expect("usage cleanup should succeed");
    assert_eq!(summary.records_deleted, 1);
    assert_eq!(summary.header_cleaned, 2);
    assert_eq!(summary.body_cleaned, 1);
    assert_eq!(summary.legacy_body_refs_migrated, 1);
    assert_eq!(summary.body_externalized, 1);
    assert_eq!(summary.keys_cleaned, 2);

    let deleted_log: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM \"usage\" WHERE request_id = 'cleanup-log'")
            .fetch_one(&pool)
            .await
            .expect("deleted usage should count");
    assert_eq!(deleted_log, 0);
    let stale_fields: (Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT request_headers, request_body_compressed FROM \"usage\" WHERE request_id = 'cleanup-stale-body'",
    )
    .fetch_one(&pool)
    .await
    .expect("stale usage should load");
    assert_eq!(stale_fields, (None, None));
    let stale_blobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_body_blobs WHERE request_id = 'cleanup-stale-body'",
    )
    .fetch_one(&pool)
    .await
    .expect("stale blobs should count");
    assert_eq!(stale_blobs, 0);

    let detail_blob: Vec<u8> = sqlx::query_scalar(
        "SELECT payload_gzip FROM usage_body_blobs WHERE request_id = 'cleanup-detail'",
    )
    .fetch_one(&pool)
    .await
    .expect("externalized body should load");
    assert_eq!(
        super::inflate_usage_json_value(&detail_blob).expect("body gzip should decode"),
        serde_json::json!({"detail": true})
    );
    let detail_inline: Option<String> = sqlx::query_scalar(
        "SELECT request_body FROM \"usage\" WHERE request_id = 'cleanup-detail'",
    )
    .fetch_one(&pool)
    .await
    .expect("detail inline body should load");
    assert!(detail_inline.is_none());

    let legacy_metadata: String = sqlx::query_scalar(
        "SELECT request_metadata FROM \"usage\" WHERE request_id = 'cleanup-legacy'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy metadata should load");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&legacy_metadata).expect("valid metadata"),
        serde_json::json!({"trace": "kept"})
    );
    let legacy_ref: String = sqlx::query_scalar(
        "SELECT request_body_ref FROM usage_http_audits WHERE request_id = 'cleanup-legacy'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy ref should migrate");
    assert_eq!(
        legacy_ref,
        "usage://request/cleanup-legacy/request_body".to_string()
    );

    let disabled_key: i64 =
        sqlx::query_scalar("SELECT is_active FROM api_keys WHERE id = 'cleanup-disable-key'")
            .fetch_one(&pool)
            .await
            .expect("disabled key should remain");
    assert_eq!(disabled_key, 0);
    let deleted_key: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM api_keys WHERE id = 'cleanup-delete-key'")
            .fetch_one(&pool)
            .await
            .expect("deleted key should count");
    assert_eq!(deleted_key, 0);
    let wallet_status: String =
        sqlx::query_scalar("SELECT status FROM wallets WHERE id = 'cleanup-delete-wallet'")
            .fetch_one(&pool)
            .await
            .expect("expired key wallet should load");
    assert_eq!(wallet_status, "disabled");

    let new_fields: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT request_headers, request_body FROM \"usage\" WHERE request_id = 'cleanup-new'",
    )
    .fetch_one(&pool)
    .await
    .expect("new usage should load");
    assert!(new_fields.0.is_some());
    assert!(new_fields.1.is_some());
}

#[tokio::test]
async fn sqlite_usage_cleanup_before_now_only_clears_selected_body_fields() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let repository = SqliteUsageWriteRepository::new(pool.clone());
    repository
        .upsert(sample_usage(
            "cleanup-before-now",
            "completed",
            "settled",
            10,
        ))
        .await
        .expect("usage should seed");
    sqlx::query(
        r#"
UPDATE "usage"
SET request_headers = '{"keep":true}',
    request_body = '{"raw":true}',
    request_body_compressed = X'1F8B'
WHERE request_id = 'cleanup-before-now';
INSERT INTO usage_body_blobs (body_ref, request_id, body_field, payload_gzip)
VALUES (
  'usage-body://cleanup-before-now/request_body',
  'cleanup-before-now', 'request_body', X'1F8B'
);
INSERT INTO usage_http_audits (
  request_id, request_headers, request_body_ref, body_capture_mode
)
VALUES (
  'cleanup-before-now', '{"keep":true}',
  'usage-body://cleanup-before-now/request_body', 'ref_backed'
);
"#,
    )
    .execute(&pool)
    .await
    .expect("body fixtures should seed");

    let window = cleanup_window(20, 20, 20, 20);
    let preview = repository
        .preview_usage_cleanup(
            &window,
            UsageCleanupTargets::all_policy_targets(),
            UsageCleanupExecutionMode::BeforeNowBodyFields,
        )
        .await
        .expect("cleanup preview should load");
    assert_eq!(preview.detail, 1);
    assert_eq!(preview.compressed, 1);
    assert_eq!(preview.header, 0);
    assert_eq!(preview.log, 0);

    let summary = repository
        .cleanup_usage(
            &window,
            1,
            false,
            UsageCleanupTargets::all_policy_targets(),
            UsageCleanupExecutionMode::BeforeNowBodyFields,
        )
        .await
        .expect("before-now cleanup should succeed");
    assert_eq!(summary.body_externalized, 1);
    assert_eq!(summary.body_cleaned, 1);
    assert_eq!(summary.header_cleaned, 0);
    assert_eq!(summary.records_deleted, 0);
    assert_eq!(summary.keys_cleaned, 0);

    let fields: (Option<String>, Option<Vec<u8>>, Option<String>) = sqlx::query_as(
        "SELECT request_body, request_body_compressed, request_headers FROM \"usage\" WHERE request_id = 'cleanup-before-now'",
    )
    .fetch_one(&pool)
    .await
    .expect("cleaned usage should load");
    assert!(fields.0.is_none());
    assert!(fields.1.is_none());
    assert!(fields.2.is_some());
    let audit_headers: Option<String> = sqlx::query_scalar(
        "SELECT request_headers FROM usage_http_audits WHERE request_id = 'cleanup-before-now'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit headers should remain");
    assert!(audit_headers.is_some());
}

#[tokio::test]
async fn sqlite_usage_write_repository_does_not_regress_void_usage() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool.clone());
    repository
        .upsert(sample_usage("request-1", "failed", "void", 1_000))
        .await
        .expect("void usage should upsert");
    let existing = repository
        .upsert(sample_usage("request-1", "pending", "pending", 1_001))
        .await
        .expect("stale usage should be ignored");

    assert_eq!(existing.status, "failed");
    assert_eq!(existing.billing_status, "void");
    assert_eq!(existing.updated_at_unix_secs, 1_000);
}

#[tokio::test]
async fn sqlite_usage_write_repository_does_not_reopen_void_failure_from_late_streaming() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool);
    for (request_id, status_code) in [
        ("request-late-active", None),
        ("request-late-response-start", Some(200)),
    ] {
        let mut failed = sample_usage(request_id, "failed", "void", 1_000);
        failed.status_code = Some(503);
        repository
            .upsert(failed)
            .await
            .expect("failed usage should upsert");

        let mut late_streaming = sample_usage(request_id, "streaming", "pending", 1_001);
        late_streaming.status_code = status_code;
        late_streaming.finalized_at_unix_secs = None;
        let current = repository
            .upsert(late_streaming)
            .await
            .expect("late streaming usage should be ignored");

        assert_eq!(current.status, "failed");
        assert_eq!(current.billing_status, "void");
        assert_eq!(current.status_code, Some(503));
        assert_eq!(current.finalized_at_unix_secs, Some(1_000));
    }
}

#[tokio::test]
async fn sqlite_usage_write_repository_does_not_regress_terminal_usage_from_late_streaming() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool.clone());
    let mut terminal = sample_usage("request-1", "completed", "pending", 1_000);
    terminal.request_metadata = Some(serde_json::json!({"trace_id": "terminal-trace"}));
    repository
        .upsert(terminal)
        .await
        .expect("terminal usage should upsert");

    let mut late_streaming = sample_usage("request-1", "streaming", "pending", 1_001);
    late_streaming.input_tokens = Some(0);
    late_streaming.output_tokens = Some(0);
    late_streaming.total_tokens = Some(0);
    late_streaming.cache_read_input_tokens = Some(0);
    late_streaming.cache_read_cost_usd = Some(0.0);
    late_streaming.total_cost_usd = Some(0.0);
    late_streaming.actual_total_cost_usd = Some(0.0);
    late_streaming.response_time_ms = Some(9_999);
    late_streaming.first_byte_time_ms = Some(9_999);
    late_streaming.finalized_at_unix_secs = None;
    late_streaming.provider_name = "Late Provider".to_string();
    late_streaming.model = "late-model".to_string();
    late_streaming.target_model = Some("late-target".to_string());
    late_streaming.request_type = Some("late-request".to_string());
    late_streaming.api_format = Some("late:api".to_string());
    late_streaming.api_family = Some("late-family".to_string());
    late_streaming.endpoint_kind = Some("late-endpoint".to_string());
    late_streaming.endpoint_api_format = Some("late:endpoint".to_string());
    late_streaming.provider_api_family = Some("late-provider-family".to_string());
    late_streaming.provider_endpoint_kind = Some("late-provider-endpoint".to_string());
    late_streaming.has_format_conversion = Some(false);
    late_streaming.is_stream = Some(true);
    late_streaming.candidate_id = Some("late-candidate".to_string());
    late_streaming.candidate_index = Some(99);
    late_streaming.key_name = Some("late-key".to_string());
    late_streaming.planner_kind = Some("late-planner".to_string());
    late_streaming.route_family = Some("late-route-family".to_string());
    late_streaming.route_kind = Some("late-route-kind".to_string());
    late_streaming.execution_path = Some("late-path".to_string());
    late_streaming.local_execution_runtime_miss_reason = Some("late-miss".to_string());
    late_streaming.request_metadata = Some(serde_json::json!({
        "provider_service_tier": "priority",
        "upstream_is_stream": true
    }));

    let current = repository
        .upsert(late_streaming)
        .await
        .expect("late streaming usage should not regress terminal usage");

    assert_eq!(current.status, "completed");
    assert_eq!(current.billing_status, "pending");
    assert_eq!(current.total_tokens, 5);
    assert_eq!(current.cache_read_input_tokens, 2);
    assert_eq!(current.total_cost_usd, 0.5);
    assert_eq!(current.actual_total_cost_usd, 0.4);
    assert_eq!(current.response_time_ms, Some(42));
    assert_eq!(current.first_byte_time_ms, Some(12));
    assert_eq!(current.finalized_at_unix_secs, Some(1_000));
    assert_eq!(current.updated_at_unix_secs, 1_000);
    assert_eq!(current.provider_name, "Provider One");
    assert_eq!(current.model, "model-1");
    assert_eq!(current.target_model.as_deref(), Some("target-model"));
    assert_eq!(current.request_type.as_deref(), Some("chat"));
    assert_eq!(current.api_format.as_deref(), Some("openai"));
    assert!(current.has_format_conversion);
    assert!(!current.is_stream);
    assert_eq!(current.candidate_id.as_deref(), Some("candidate-1"));
    assert_eq!(current.candidate_index, Some(1));
    assert_eq!(current.key_name.as_deref(), Some("key-one"));
    assert_eq!(current.planner_kind.as_deref(), Some("default"));
    assert_eq!(current.route_family.as_deref(), Some("chat"));
    assert_eq!(current.route_kind.as_deref(), Some("completion"));
    assert_eq!(current.execution_path.as_deref(), Some("remote"));
    assert_eq!(current.provider_service_tier(), None);
    assert_eq!(
        current
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("trace_id"))
            .and_then(serde_json::Value::as_str),
        Some("terminal-trace")
    );

    let listed = SqliteUsageReadRepository::new(pool)
        .list_usage_audits(&UsageAuditListQuery {
            limit: Some(10),
            newest_first: true,
            ..UsageAuditListQuery::default()
        })
        .await
        .expect("usage list should load")
        .into_iter()
        .find(|item| item.request_id == "request-1")
        .expect("terminal usage should be listed");
    assert_eq!(listed.provider_name, "Provider One");
    assert_eq!(listed.model, "model-1");
    assert_eq!(listed.candidate_id.as_deref(), Some("candidate-1"));
    assert_eq!(listed.provider_service_tier(), None);
}

#[tokio::test]
async fn sqlite_usage_write_repository_allows_authoritative_completed_recovery() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool);
    repository
        .upsert(sample_usage("request-recovery", "failed", "void", 1_000))
        .await
        .expect("void failure should upsert");

    let mut recovery = sample_usage("request-recovery", "completed", "pending", 1_001);
    recovery.provider_name = "Recovered Provider".to_string();
    recovery.model = "recovered-model".to_string();
    recovery.target_model = Some("recovered-target".to_string());
    recovery.api_format = Some("recovered:api".to_string());
    recovery.candidate_id = Some("recovered-candidate".to_string());
    recovery.request_metadata = Some(serde_json::json!({"provider_service_tier": "priority"}));
    let recovered = repository
        .upsert(recovery)
        .await
        .expect("completed recovery should upsert");

    assert_eq!(recovered.status, "completed");
    assert_eq!(recovered.billing_status, "pending");
    assert_eq!(recovered.provider_name, "Recovered Provider");
    assert_eq!(recovered.model, "recovered-model");
    assert_eq!(recovered.target_model.as_deref(), Some("recovered-target"));
    assert_eq!(recovered.api_format.as_deref(), Some("recovered:api"));
    assert_eq!(
        recovered.candidate_id.as_deref(),
        Some("recovered-candidate")
    );
    assert_eq!(
        recovered.provider_service_tier().as_deref(),
        Some("priority")
    );
}

#[tokio::test]
async fn sqlite_usage_write_repository_preserves_streaming_response_start_from_late_active() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool);
    repository
        .upsert(sample_usage(
            "request-late-active",
            "streaming",
            "pending",
            1_000,
        ))
        .await
        .expect("response-start usage should upsert");

    let mut late_active = sample_usage("request-late-active", "streaming", "pending", 1_001);
    late_active.status_code = None;
    late_active.response_time_ms = None;
    late_active.first_byte_time_ms = None;

    let current = repository
        .upsert(late_active)
        .await
        .expect("late active usage should not clear response-start fields");

    assert_eq!(current.status, "streaming");
    assert_eq!(current.status_code, Some(200));
    assert_eq!(current.response_time_ms, Some(42));
    assert_eq!(current.first_byte_time_ms, Some(12));
}

#[tokio::test]
async fn sqlite_usage_write_repository_keeps_streaming_capture_from_late_pending() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool);
    let mut streaming = sample_usage("request-streaming-capture", "streaming", "pending", 1_000);
    streaming.request_metadata = Some(serde_json::json!({"trace_id": "streaming-final"}));
    repository
        .upsert(streaming)
        .await
        .expect("streaming usage should upsert");

    let mut late_pending = sample_usage("request-streaming-capture", "pending", "pending", 1_001);
    late_pending.provider_name = "Late Provider".to_string();
    late_pending.model = "late-model".to_string();
    late_pending.candidate_id = Some("late-candidate".to_string());
    late_pending.request_metadata = Some(serde_json::json!({"provider_service_tier": "priority"}));
    let current = repository
        .upsert(late_pending)
        .await
        .expect("late pending usage should not regress streaming capture");

    assert_eq!(current.status, "streaming");
    assert_eq!(current.provider_name, "Provider One");
    assert_eq!(current.model, "model-1");
    assert_eq!(current.candidate_id.as_deref(), Some("candidate-1"));
    assert_eq!(current.provider_service_tier(), None);
    assert_eq!(
        current
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("trace_id"))
            .and_then(serde_json::Value::as_str),
        Some("streaming-final")
    );
}

#[tokio::test]
async fn sqlite_usage_write_repository_cleans_stale_pending_requests() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool.clone());
    repository
        .upsert(sample_usage("request-recovered", "streaming", "pending", 1))
        .await
        .expect("streaming usage should upsert");
    repository
        .upsert(sample_usage("request-failed", "pending", "pending", 1))
        .await
        .expect("pending usage should upsert");

    sqlx::query(
        r#"
INSERT INTO request_candidates (
  id,
  request_id,
  candidate_index,
  retry_index,
  status,
  is_cached,
  created_at
) VALUES
  ('candidate-recovered', 'request-recovered', 0, 0, 'streaming', 0, 1),
  ('candidate-failed', 'request-failed', 0, 0, 'pending', 0, 1)
"#,
    )
    .execute(&pool)
    .await
    .expect("request candidates should seed");

    let summary = repository
        .cleanup_stale_pending_requests(2, 10, 5, 1)
        .await
        .expect("cleanup should run");
    assert_eq!(summary.recovered, 1);
    assert_eq!(summary.failed, 1);

    let recovered = repository
        .find_by_request_id("request-recovered")
        .await
        .expect("recovered usage should load")
        .expect("recovered usage should exist");
    assert_eq!(recovered.status, "completed");
    assert_eq!(recovered.status_code, Some(200));

    let failed = repository
        .find_by_request_id("request-failed")
        .await
        .expect("failed usage should load")
        .expect("failed usage should exist");
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.status_code, Some(504));
    assert_eq!(failed.billing_status, "void");
    assert_eq!(failed.total_cost_usd, 0.0);
    assert_eq!(failed.finalized_at_unix_secs, Some(10));

    let candidate_statuses = sqlx::query_as::<_, (String, String, Option<i64>)>(
        r#"
SELECT request_id, status, finished_at
FROM request_candidates
ORDER BY request_id
"#,
    )
    .fetch_all(&pool)
    .await
    .expect("candidate statuses should load");
    assert_eq!(
        candidate_statuses,
        vec![
            (
                "request-failed".to_string(),
                "failed".to_string(),
                Some(10_000)
            ),
            (
                "request-recovered".to_string(),
                "success".to_string(),
                Some(10_000)
            ),
        ]
    );

    let snapshot = sqlx::query_as::<_, (String, Option<i64>)>(
            "SELECT billing_status, finalized_at FROM usage_settlement_snapshots WHERE request_id = 'request-failed'",
        )
        .fetch_one(&pool)
        .await
        .expect("void settlement snapshot should load");
    assert_eq!(snapshot, ("void".to_string(), Some(10)));
}

#[tokio::test]
async fn sqlite_usage_write_repository_cleanup_uses_failed_candidate_status_when_present() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool.clone());
    repository
        .upsert(sample_usage(
            "request-upstream-reset",
            "pending",
            "pending",
            1,
        ))
        .await
        .expect("pending usage should upsert");
    repository
        .upsert(sample_usage("request-stuck", "pending", "pending", 1))
        .await
        .expect("pending usage should upsert");

    // request-upstream-reset has a failed candidate carrying a concrete 502 status
    // and a connection-reset message — cleanup should use them instead of 504.
    // request-stuck has only a still-pending candidate, so cleanup should fall back to 504.
    sqlx::query(
            r#"
INSERT INTO request_candidates (
  id,
  request_id,
  candidate_index,
  retry_index,
  status,
  status_code,
  error_message,
  is_cached,
  created_at,
  started_at,
  finished_at
) VALUES
  ('candidate-reset', 'request-upstream-reset', 0, 0, 'failed', 502, 'upstream connection reset by peer', 0, 1, 2, 3),
  ('candidate-stuck', 'request-stuck', 0, 0, 'pending', NULL, NULL, 0, 1, NULL, NULL)
"#,
        )
        .execute(&pool)
        .await
        .expect("request candidates should seed");

    let summary = repository
        .cleanup_stale_pending_requests(2, 10, 5, 5)
        .await
        .expect("cleanup should run");
    assert_eq!(summary.recovered, 0);
    assert_eq!(summary.failed, 2);

    let reset = repository
        .find_by_request_id("request-upstream-reset")
        .await
        .expect("upstream-reset usage should load")
        .expect("upstream-reset usage should exist");
    assert_eq!(reset.status, "failed");
    assert_eq!(reset.status_code, Some(502));
    assert_eq!(
        reset.error_message.as_deref(),
        Some("upstream connection reset by peer")
    );

    let stuck = repository
        .find_by_request_id("request-stuck")
        .await
        .expect("stuck usage should load")
        .expect("stuck usage should exist");
    assert_eq!(stuck.status, "failed");
    assert_eq!(stuck.status_code, Some(504));
    assert!(stuck
        .error_message
        .as_deref()
        .is_some_and(|message| message.contains("超过 5 分钟未完成")));
}

#[tokio::test]
async fn sqlite_usage_read_repository_reads_usage_contract_views() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let writer = SqliteUsageWriteRepository::new(pool.clone());
    writer
        .upsert(sample_usage("request-1", "completed", "settled", 1_000))
        .await
        .expect("usage should upsert");
    writer
        .upsert(sample_usage("request-2", "failed", "void", 1_010))
        .await
        .expect("usage should upsert");
    sqlx::query(
        r#"
UPDATE "usage"
SET username = 'legacy-user', api_key_name = 'legacy-key'
WHERE request_id = 'request-1'
"#,
    )
    .execute(&pool)
    .await
    .expect("legacy display fields should seed");

    let reader = SqliteUsageReadRepository::new(pool);
    let loaded = reader
        .find_by_request_id("request-1")
        .await
        .expect("usage should load")
        .expect("usage should exist");
    assert_eq!(loaded.total_tokens, 5);
    assert_eq!(loaded.billing_status, "settled");
    assert_eq!(loaded.username.as_deref(), Some("legacy-user"));
    assert_eq!(loaded.api_key_name.as_deref(), Some("legacy-key"));

    let listed = reader
        .list_usage_audits(&UsageAuditListQuery {
            provider_name: Some("Provider One".to_string()),
            newest_first: true,
            ..UsageAuditListQuery::default()
        })
        .await
        .expect("usage list should load");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].request_id, "request-2");

    let summary = reader
        .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
            created_from_unix_secs: 999,
            created_until_unix_secs: 1_020,
            user_id: Some("user-1".to_string()),
        })
        .await
        .expect("dashboard summary should load");
    assert_eq!(summary.total_requests, 2);
    assert_eq!(summary.error_requests, 1);
    assert_eq!(summary.total_tokens, 10);
}

#[tokio::test]
async fn sqlite_usage_daily_heatmap_reads_imported_daily_aggregates() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    sqlx::query(
        r#"
INSERT INTO stats_daily (
    id, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, actual_total_cost, is_complete, created_at, updated_at
) VALUES (
    'daily-1', 86400, 9, 8, 1, 10, 20, 3, 4, 1.25, 1.0, 1, 1, 1
);
INSERT INTO stats_user_daily (
    id, user_id, username, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
) VALUES (
    'user-daily-1', 'user-1', 'user one', 86400, 5, 5, 0, 7, 8, 2, 1, 0.75, 1, 1
);
"#,
    )
    .execute(&pool)
    .await
    .expect("daily aggregates should seed");

    let reader = SqliteUsageReadRepository::new(pool);
    let admin = reader
        .summarize_usage_daily_heatmap(&UsageDailyHeatmapQuery {
            created_from_unix_secs: 0,
            user_id: None,
            admin_mode: true,
        })
        .await
        .expect("admin heatmap should load");
    assert_eq!(admin.len(), 1);
    assert_eq!(admin[0].date, "1970-01-02");
    assert_eq!(admin[0].requests, 9);
    assert_eq!(admin[0].total_tokens, 37);
    assert_eq!(admin[0].actual_total_cost_usd, 1.0);

    let user = reader
        .summarize_usage_daily_heatmap(&UsageDailyHeatmapQuery {
            created_from_unix_secs: 0,
            user_id: Some("user-1".to_string()),
            admin_mode: false,
        })
        .await
        .expect("user heatmap should load");
    assert_eq!(user.len(), 1);
    assert_eq!(user[0].date, "1970-01-02");
    assert_eq!(user[0].requests, 5);
    assert_eq!(user[0].total_tokens, 18);
    assert_eq!(user[0].actual_total_cost_usd, 0.75);
}

#[tokio::test]
async fn sqlite_usage_totals_by_user_ids_reads_imported_user_daily_aggregates() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    sqlx::query(
        r#"
INSERT INTO stats_user_daily (
    id, user_id, username, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
) VALUES (
    'user-daily-1', 'user-1', 'user one', 86400, 5, 5, 0, 7, 8, 2, 1, 0.75, 1, 1
);
INSERT INTO "usage" (
    request_id, id, user_id, api_key_id, provider_name, model, total_tokens,
    status, billing_status, created_at_unix_ms, updated_at_unix_secs
) VALUES
    ('raw-before-cutoff', 'usage-1', 'user-1', 'api-key-1', 'Provider One', 'model-1', 99,
     'completed', 'settled', 90000, 90000),
    ('raw-after-cutoff', 'usage-2', 'user-1', 'api-key-1', 'Provider One', 'model-1', 7,
     'completed', 'settled', 172800, 172800);
"#,
    )
    .execute(&pool)
    .await
    .expect("usage totals fixtures should seed");

    let reader = SqliteUsageReadRepository::new(pool);
    let totals = reader
        .summarize_usage_totals_by_user_ids(&["user-1".to_string()])
        .await
        .expect("user totals should load");

    assert_eq!(totals.len(), 1);
    assert_eq!(totals[0].user_id, "user-1");
    assert_eq!(totals[0].request_count, 6);
    assert_eq!(totals[0].total_tokens, 25);
}

#[tokio::test]
async fn sqlite_dashboard_daily_stats_reads_imported_daily_aggregates() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    sqlx::query(
        r#"
INSERT INTO stats_daily (
    id, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, actual_total_cost, is_complete, created_at, updated_at
) VALUES (
    'daily-1', 86400, 9, 8, 1, 10, 20, 3, 4, 1.25, 1.0, 1, 1, 1
);
"#,
    )
    .execute(&pool)
    .await
    .expect("daily aggregates should seed");

    let reader = SqliteUsageReadRepository::new(pool);
    let summary = reader
        .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 172800,
            user_id: None,
        })
        .await
        .expect("dashboard summary should load");
    assert_eq!(summary.total_requests, 9);
    assert_eq!(summary.total_tokens, 37);

    let rows = reader
        .list_dashboard_daily_breakdown(&UsageDashboardDailyBreakdownQuery {
            created_from_unix_secs: 0,
            created_until_unix_secs: 172800,
            tz_offset_minutes: 480,
            user_id: None,
        })
        .await
        .expect("dashboard daily breakdown should load");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].date, "1970-01-02");
    assert_eq!(rows[0].model, "aggregate");
    assert_eq!(rows[0].requests, 9);
    assert_eq!(rows[0].total_tokens, 37);
}

#[tokio::test]
async fn sqlite_first_byte_fast_path_preserves_lifecycle_state_and_counters() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let repository = SqliteUsageWriteRepository::new(pool.clone());

    assert!(repository.supports_first_byte_usage_fast_path());
    assert!(repository.supports_first_byte_usage_batch());

    for request_id in ["first-byte-duplicate", "first-byte-unique"] {
        let mut pending = sample_usage(request_id, "pending", "pending", 1_000);
        pending.status_code = None;
        pending.response_time_ms = None;
        pending.first_byte_time_ms = None;
        pending.finalized_at_unix_secs = None;
        pending.is_stream = Some(true);
        pending.request_body = Some(serde_json::json!({"prompt": request_id}));
        pending.request_body_state = Some(UsageBodyCaptureState::Inline);
        pending.request_metadata = Some(serde_json::json!({
            "trace_id": format!("pending-{request_id}"),
            "upstream_is_stream": false
        }));
        repository
            .upsert(pending)
            .await
            .expect("pending usage should seed");
    }

    let mut terminal = sample_usage("first-byte-terminal", "completed", "pending", 1_000);
    terminal.first_byte_time_ms = Some(44);
    terminal.request_metadata = Some(serde_json::json!({"trace_id": "terminal"}));
    repository
        .upsert(terminal)
        .await
        .expect("terminal usage should seed");

    let mut first = sample_usage("first-byte-duplicate", "streaming", "pending", 1_001);
    first.first_byte_time_ms = Some(30);
    first.response_time_ms = Some(31);
    first.finalized_at_unix_secs = None;
    first.request_metadata = Some(serde_json::json!({"trace_id": "incoming-first"}));
    let mut replay = first.clone();
    replay.first_byte_time_ms = Some(7);
    replay.response_time_ms = Some(99);

    let mut unique = sample_usage("first-byte-unique", "streaming", "pending", 1_001);
    unique.first_byte_time_ms = Some(18);
    unique.finalized_at_unix_secs = None;
    unique.request_metadata = Some(serde_json::json!({"trace_id": "incoming-unique"}));

    let mut late_terminal = sample_usage("first-byte-terminal", "streaming", "pending", 1_002);
    late_terminal.first_byte_time_ms = Some(3);
    late_terminal.finalized_at_unix_secs = None;
    late_terminal.request_metadata = Some(serde_json::json!({"trace_id": "late"}));

    let mut missing = sample_usage("first-byte-missing", "streaming", "pending", 1_001);
    missing.first_byte_time_ms = Some(12);
    missing.finalized_at_unix_secs = None;
    missing.request_metadata = Some(serde_json::json!({
        "trace_id": "missing",
        "upstream_is_stream": false
    }));

    repository
        .upsert_first_byte_many(vec![first, unique, late_terminal, replay, missing])
        .await
        .expect("first-byte batch should persist");

    let duplicate = repository
        .find_by_request_id("first-byte-duplicate")
        .await
        .expect("duplicate request should load")
        .expect("duplicate request should exist");
    assert_eq!(duplicate.status, "streaming");
    assert_eq!(duplicate.first_byte_time_ms, Some(30));
    assert_eq!(duplicate.response_time_ms, Some(99));
    assert_eq!(
        duplicate.request_metadata.as_ref().unwrap()["trace_id"],
        "pending-first-byte-duplicate"
    );
    assert_eq!(
        duplicate.request_body,
        Some(serde_json::json!({"prompt": "first-byte-duplicate"}))
    );

    let unique = repository
        .find_by_request_id("first-byte-unique")
        .await
        .expect("unique request should load")
        .expect("unique request should exist");
    assert_eq!(unique.status, "streaming");
    assert_eq!(unique.first_byte_time_ms, Some(18));
    assert_eq!(
        unique.request_metadata.as_ref().unwrap()["trace_id"],
        "pending-first-byte-unique"
    );

    let terminal = repository
        .find_by_request_id("first-byte-terminal")
        .await
        .expect("terminal request should load")
        .expect("terminal request should exist");
    assert_eq!(terminal.status, "completed");
    assert_eq!(terminal.first_byte_time_ms, Some(44));
    assert_eq!(
        terminal.request_metadata.as_ref().unwrap()["trace_id"],
        "terminal"
    );

    let missing = repository
        .find_by_request_id("first-byte-missing")
        .await
        .expect("missing request should load")
        .expect("missing request should have been inserted");
    assert_eq!(missing.status, "streaming");
    assert_eq!(missing.billing_status, "pending");
    assert_eq!(missing.first_byte_time_ms, Some(12));
    assert_eq!(
        missing.request_metadata.as_ref().unwrap()["upstream_is_stream"],
        false
    );

    let missing_counter_delta: i64 = sqlx::query_scalar(
        r#"
SELECT COALESCE(SUM(request_count_delta), 0)
FROM usage_counter_deltas
WHERE request_id = 'first-byte-missing'
  AND kind = 'provider_api_key'
  AND target_id = 'provider-key-1'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("missing first-byte counter delta should load");
    assert_eq!(missing_counter_delta, 1);
}

#[tokio::test]
async fn sqlite_pending_batch_is_atomic_and_persists_auxiliary_state() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;
    let repository = SqliteUsageWriteRepository::new(pool.clone());
    assert!(repository.supports_pending_usage_batch());

    let mut first = sample_usage("pending-batch-first", "pending", "pending", 1_000);
    first.finalized_at_unix_secs = None;
    first.request_headers = Some(serde_json::json!({"x-request": "first"}));
    first.request_body = Some(serde_json::json!({"prompt": "first"}));
    first.request_body_state = Some(UsageBodyCaptureState::Inline);
    let mut second = sample_usage("pending-batch-second", "pending", "pending", 1_001);
    second.finalized_at_unix_secs = None;
    second.request_headers = Some(serde_json::json!({"x-request": "second"}));

    sqlx::query(
        r#"
CREATE TRIGGER reject_second_pending_audit
BEFORE INSERT ON usage_http_audits
WHEN NEW.request_id = 'pending-batch-second'
BEGIN
  SELECT RAISE(ABORT, 'reject pending batch test row');
END
"#,
    )
    .execute(&pool)
    .await
    .expect("rollback trigger should install");
    repository
        .upsert_pending_many(vec![first.clone(), second.clone()])
        .await
        .expect_err("auxiliary write failure should roll back the pending batch");
    let rolled_back_usage: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM \"usage\" WHERE request_id LIKE 'pending-batch-%'",
    )
    .fetch_one(&pool)
    .await
    .expect("rolled back usage should count");
    let rolled_back_deltas: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_counter_deltas WHERE request_id LIKE 'pending-batch-%'",
    )
    .fetch_one(&pool)
    .await
    .expect("rolled back deltas should count");
    assert_eq!(rolled_back_usage, 0);
    assert_eq!(rolled_back_deltas, 0);

    sqlx::query("DROP TRIGGER reject_second_pending_audit")
        .execute(&pool)
        .await
        .expect("rollback trigger should drop");
    repository
        .upsert_pending_many(vec![first, second])
        .await
        .expect("pending batch should commit");

    let committed: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
SELECT
  (SELECT COUNT(*) FROM "usage" WHERE request_id LIKE 'pending-batch-%'),
  (SELECT COUNT(*) FROM usage_http_audits WHERE request_id LIKE 'pending-batch-%'),
  (SELECT COUNT(*) FROM usage_body_blobs WHERE request_id LIKE 'pending-batch-%'),
  (SELECT COUNT(*) FROM usage_routing_snapshots WHERE request_id LIKE 'pending-batch-%'),
  (SELECT COUNT(*) FROM usage_settlement_snapshots WHERE request_id LIKE 'pending-batch-%')
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("pending batch auxiliary rows should count");
    assert_eq!(committed, (2, 2, 1, 2, 2));

    let provider_deltas: i64 = sqlx::query_scalar(
        r#"
SELECT COALESCE(SUM(request_count_delta), 0)
FROM usage_counter_deltas
WHERE request_id LIKE 'pending-batch-%' AND kind = 'provider_api_key'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("pending batch provider deltas should load");
    assert_eq!(provider_deltas, 2);
}

#[tokio::test]
async fn sqlite_concurrent_same_request_upserts_enqueue_counters_once() {
    let database_path = std::env::temp_dir().join(format!(
        "aether-usage-counter-concurrency-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(30));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_stats_targets(&pool).await;

    let repository = SqliteUsageWriteRepository::new(pool.clone());
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let repository = repository.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let usage = sample_usage("concurrent-counter-request", "completed", "pending", 1_000);
            barrier.wait().await;
            repository.upsert(usage).await
        }));
    }
    for task in tasks {
        task.await
            .expect("concurrent usage writer should join")
            .expect("concurrent usage should persist");
    }

    repository
        .flush_usage_counter_deltas(100)
        .await
        .expect("usage counter deltas should flush");
    let api_key_requests: i64 =
        sqlx::query_scalar("SELECT total_requests FROM api_keys WHERE id = 'api-key-1'")
            .fetch_one(&pool)
            .await
            .expect("api key counter should load");
    let provider_key_requests: i64 = sqlx::query_scalar(
        "SELECT request_count FROM provider_api_keys WHERE id = 'provider-key-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("provider key counter should load");
    let model_requests: i64 =
        sqlx::query_scalar("SELECT usage_count FROM global_models WHERE name = 'model-1'")
            .fetch_one(&pool)
            .await
            .expect("model counter should load");
    let outbox_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_counter_deltas")
        .fetch_one(&pool)
        .await
        .expect("usage counter outbox should load");
    let routing_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_routing_snapshots WHERE request_id = 'concurrent-counter-request'",
    )
    .fetch_one(&pool)
    .await
    .expect("routing snapshot should count");
    let settlement_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_settlement_snapshots WHERE request_id = 'concurrent-counter-request'",
    )
    .fetch_one(&pool)
    .await
    .expect("settlement snapshot should count");
    assert_eq!(api_key_requests, 1);
    assert_eq!(provider_key_requests, 1);
    assert_eq!(model_requests, 1);
    assert_eq!(outbox_rows, 3);
    assert_eq!(routing_rows, 1);
    assert_eq!(settlement_rows, 1);

    drop(repository);
    pool.close().await;
    let _ = std::fs::remove_file(&database_path);
    let _ = std::fs::remove_file(format!("{}-wal", database_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", database_path.display()));
}

async fn seed_stats_targets(pool: &sqlx::SqlitePool) {
    sqlx::query(
        r#"
INSERT INTO users (id, auth_source, created_at, updated_at)
VALUES ('user-1', 'local', 1, 1);
INSERT INTO api_keys (id, user_id, key_hash, created_at, updated_at)
VALUES ('api-key-1', 'user-1', 'hash-1', 1, 1);
INSERT INTO providers (id, name, provider_type, created_at, updated_at)
VALUES ('provider-1', 'Provider One', 'openai', 1, 1);
INSERT INTO provider_api_keys (id, provider_id, name, created_at, updated_at)
VALUES ('provider-key-1', 'provider-1', 'Provider Key One', 1, 1);
INSERT INTO global_models (id, name, created_at, updated_at)
VALUES ('global-model-1', 'model-1', 1, 1);
"#,
    )
    .execute(pool)
    .await
    .expect("stats targets should seed");
}

fn sample_usage(
    request_id: &str,
    status: &str,
    billing_status: &str,
    updated_at: u64,
) -> UpsertUsageRecord {
    UpsertUsageRecord {
        request_id: request_id.to_string(),
        user_id: Some("user-1".to_string()),
        api_key_id: Some("api-key-1".to_string()),
        username: Some("legacy-user".to_string()),
        api_key_name: Some("legacy-key".to_string()),
        provider_name: "Provider One".to_string(),
        model: "model-1".to_string(),
        target_model: Some("target-model".to_string()),
        provider_id: Some("provider-1".to_string()),
        provider_endpoint_id: Some("endpoint-1".to_string()),
        provider_api_key_id: Some("provider-key-1".to_string()),
        request_type: Some("chat".to_string()),
        api_format: Some("openai".to_string()),
        api_family: Some("chat".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_api_format: Some("openai".to_string()),
        provider_api_family: Some("chat".to_string()),
        provider_endpoint_kind: Some("chat".to_string()),
        has_format_conversion: Some(true),
        is_stream: Some(false),
        input_tokens: Some(2),
        output_tokens: Some(3),
        total_tokens: None,
        cache_creation_input_tokens: None,
        cache_creation_ephemeral_5m_input_tokens: Some(0),
        cache_creation_ephemeral_1h_input_tokens: Some(0),
        cache_read_input_tokens: Some(2),
        cache_creation_cost_usd: Some(0.0),
        cache_read_cost_usd: Some(0.1),
        output_price_per_1m: Some(2.0),
        total_cost_usd: Some(0.5),
        actual_total_cost_usd: Some(0.4),
        status_code: Some(200),
        error_message: None,
        error_category: None,
        response_time_ms: Some(42),
        first_byte_time_ms: Some(12),
        status: status.to_string(),
        billing_status: billing_status.to_string(),
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
        candidate_id: Some("candidate-1".to_string()),
        candidate_index: Some(1),
        key_name: Some("key-one".to_string()),
        planner_kind: Some("default".to_string()),
        route_family: Some("chat".to_string()),
        route_kind: Some("completion".to_string()),
        execution_path: Some("remote".to_string()),
        local_execution_runtime_miss_reason: None,
        request_metadata: Some(serde_json::json!({
            "trace_id": "trace-1",
            "upstream_is_stream": true,
        })),
        finalized_at_unix_secs: Some(updated_at),
        created_at_unix_ms: Some(updated_at),
        updated_at_unix_secs: updated_at,
    }
}

fn cleanup_window(
    detail_cutoff: i64,
    compressed_cutoff: i64,
    header_cutoff: i64,
    log_cutoff: i64,
) -> UsageCleanupWindow {
    fn timestamp(value: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(value, 0).expect("test timestamp should be valid")
    }

    UsageCleanupWindow {
        detail_cutoff: timestamp(detail_cutoff),
        compressed_cutoff: timestamp(compressed_cutoff),
        header_cutoff: timestamp(header_cutoff),
        log_cutoff: timestamp(log_cutoff),
    }
}
