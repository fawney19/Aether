use super::{SqliteUsageReadRepository, SqliteUsageWriteRepository};
use crate::run_migrations;
use aether_data_contracts::repository::usage::{
    UpsertUsageRecord, UsageAuditListQuery, UsageDailyHeatmapQuery,
    UsageDashboardDailyBreakdownQuery, UsageDashboardSummaryQuery, UsageProviderPerformanceQuery,
    UsageReadRepository, UsageTimeSeriesGranularity, UsageWriteRepository,
};

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
async fn sqlite_usage_write_repository_upserts_and_rebuilds_stats() {
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
    assert_eq!(record.total_tokens, 7);
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
    assert_eq!(stats, (1, 7, 0.5, Some(1_000)));

    let provider_stats = sqlx::query_as::<_, (i64, i64, i64, i64, f64, i64, Option<i64>)>(
            "SELECT request_count, success_count, error_count, total_tokens, total_cost_usd, total_response_time_ms, last_used_at FROM provider_api_keys WHERE id = 'provider-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("provider key stats should load");
    assert_eq!(provider_stats, (1, 1, 0, 7, 0.5, 42, Some(1_000)));
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
    assert_eq!(current.total_tokens, 7);
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

    let reader = SqliteUsageReadRepository::new(pool);
    let loaded = reader
        .find_by_request_id("request-1")
        .await
        .expect("usage should load")
        .expect("usage should exist");
    assert_eq!(loaded.total_tokens, 7);
    assert_eq!(loaded.billing_status, "settled");

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
