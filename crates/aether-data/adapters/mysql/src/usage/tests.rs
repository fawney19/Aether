use super::{MysqlUsageStorage, MysqlUsageWriteRepository};
use crate::run_migrations;
use aether_data_contracts::repository::usage::{
    UpsertUsageRecord, UsageAuditListQuery, UsageBodyCaptureState, UsageCleanupExecutionMode,
    UsageCleanupTargets, UsageCleanupWindow, UsageWriteRepository,
};
use chrono::DateTime;

#[tokio::test]
async fn repository_builds_from_lazy_pool() {
    let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
        "mysql://user:pass@localhost:3306/aether"
            .parse()
            .expect("mysql options should parse"),
    );

    let _repository = MysqlUsageWriteRepository::new(pool);
}

#[test]
fn mysql_usage_daily_heatmap_reads_imported_daily_aggregates() {
    let source = include_str!("../usage.rs");
    assert!(source.contains("summarize_usage_daily_heatmap_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("AS SIGNED) AS total_tokens"));
    assert!(source.contains("CAST(COUNT(*) AS SIGNED) AS requests"));
    assert!(source.contains("summaries.entry(item.date.clone()).or_insert(item)"));
}

#[test]
fn mysql_usage_totals_by_user_ids_reads_imported_user_daily_aggregates() {
    let source = include_str!("../usage.rs");
    assert!(source.contains("async fn summarize_usage_totals_by_user_ids"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("MAX(`date`) AS latest_date"));
    assert!(source.contains("AS SIGNED) AS request_count"));
    assert!(source.contains("CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS request_count"));
    assert!(source.contains("requested.cutoff_unix_secs"));
}

#[test]
fn mysql_dashboard_reads_imported_daily_aggregates() {
    let source = include_str!("../usage.rs");
    assert!(source.contains("summarize_dashboard_usage_from_daily_aggregates"));
    assert!(source.contains("list_dashboard_daily_breakdown_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("'aggregate' AS model"));
    assert!(source.contains("AS SIGNED) AS total_requests"));
    assert!(source.contains("CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS total_requests"));
    assert!(source.contains("CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS requests"));
}

#[test]
fn mysql_usage_stat_rebuilds_aggregate_in_sql() {
    let source = include_str!("../usage.rs").replace("\r\n", "\n");
    assert!(source.contains("UPDATE api_keys\nJOIN ("));
    assert!(source.contains("AND status NOT IN ('pending', 'streaming')"));
    assert!(source.contains("MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR"));
    assert!(source.contains("MAX(created_at_unix_ms) AS last_used_at"));
    assert!(source.contains("UPDATE provider_api_keys\nJOIN ("));
    assert!(source.contains("GROUP BY provider_api_key_id"));
    assert!(!source.contains("struct ProviderKeyStats"));
    assert!(super::MYSQL_PROVIDER_KEY_SUCCESS_FLAG_EXPR.contains("outcome_class = 'success'"));
    assert!(super::MYSQL_PROVIDER_KEY_ERROR_FLAG_EXPR.contains("outcome_class = 'service_error'"));
    assert!(super::MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR.contains(
        "COALESCE(`usage`.input_tokens, 0) - COALESCE(`usage`.cache_read_input_tokens, 0)"
    ));
    assert!(super::MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR
        .contains("GREATEST(COALESCE(`usage`.output_tokens, 0), 0)"));
    assert!(super::MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR
        .contains("NULLIF(GREATEST(COALESCE(`usage`.total_tokens, 0), 0), 0)"));
    let snapshot_position = super::MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR
        .find("settlement.billing_effective_input_tokens")
        .expect("canonical total must read settlement snapshots");
    let raw_position = super::MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR
        .find("NULLIF(GREATEST(COALESCE(`usage`.total_tokens, 0), 0), 0)")
        .expect("canonical total must preserve non-zero legacy raw totals");
    assert!(snapshot_position < raw_position);
    assert!(source.matches("canonical_total_tokens_expr =").count() >= 4);
    assert_eq!(
        source
            .matches("LEFT JOIN usage_settlement_snapshots AS settlement")
            .count(),
        4
    );
}

#[test]
fn mysql_usage_upsert_keeps_terminal_state_when_streaming_arrives_late() {
    assert!(super::UPSERT_USAGE_SQL.contains(
        "status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')"
    ));
    assert!(super::UPSERT_USAGE_SQL.contains("input_tokens = CASE"));
    assert!(super::UPSERT_USAGE_SQL.contains("status_code = CASE"));
    assert!(super::UPSERT_USAGE_SQL.contains("billing_status = CASE"));
    assert!(super::UPSERT_USAGE_SQL.contains("finalized_at = CASE"));
    assert!(super::UPSERT_USAGE_SQL.contains("updated_at_unix_secs = CASE"));
    assert!(super::UPSERT_USAGE_SQL
        .contains("WHEN status = 'streaming' AND VALUES(status) = 'pending' THEN status"));
    assert!(super::UPSERT_USAGE_SQL.contains(
        "WHEN status = 'streaming' AND VALUES(status) = 'streaming' AND VALUES(status_code) IS NULL THEN status_code"
    ));
}

#[test]
fn mysql_usage_upsert_guards_candidate_identity_metadata_and_routing_from_late_lifecycle() {
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
        let preserve = format!("THEN {field} ELSE VALUES({field}) END");
        assert!(
            super::UPSERT_USAGE_SQL.contains(&preserve),
            "late lifecycle must preserve {field}"
        );
    }
    assert!(super::UPSERT_USAGE_SQL
        .contains("OR (status = 'streaming' AND VALUES(status) = 'pending')"));
}

#[tokio::test]
async fn mysql_usage_write_repository_upserts_and_flushes_counters_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql usage write smoke test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");

    let suffix = unique_suffix();
    let user_id = format!("user-{suffix}");
    let api_key_id = format!("api-key-{suffix}");
    let provider_id = format!("provider-{suffix}");
    let provider_key_id = format!("provider-key-{suffix}");
    let model_name = format!("model-{suffix}");
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;
    sqlx::query("INSERT INTO global_models (id, name, created_at, updated_at) VALUES (?, ?, 1, 1)")
        .bind(format!("global-model-{suffix}"))
        .bind(&model_name)
        .execute(&pool)
        .await
        .expect("global model should seed");

    let repository = MysqlUsageWriteRepository::new(pool.clone());
    let mut usage = sample_usage(
        &format!("request-{suffix}"),
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "completed",
        "pending",
        1_000,
    );
    usage.model.clone_from(&model_name);
    let record = repository.upsert(usage).await.expect("usage should upsert");

    assert_eq!(record.api_key_id.as_deref(), Some(api_key_id.as_str()));
    assert_eq!(
        record.provider_api_key_id.as_deref(),
        Some(provider_key_id.as_str())
    );
    assert_eq!(record.total_tokens, 5);
    assert_eq!(
        record.request_metadata.as_ref().unwrap()["upstream_is_stream"],
        true
    );
    let upstream_is_stream: Option<bool> =
        sqlx::query_scalar("SELECT upstream_is_stream FROM `usage` WHERE request_id = ?")
            .bind(format!("request-{suffix}"))
            .fetch_one(&pool)
            .await
            .expect("usage stream mode should load");
    assert_eq!(upstream_is_stream, Some(true));

    repository
        .flush_usage_counter_deltas(100)
        .await
        .expect("usage counter deltas should flush");
    assert!(
        repository
            .rebuild_api_key_usage_stats()
            .await
            .expect("api key stats should rebuild")
            >= 1
    );
    assert!(
        repository
            .rebuild_provider_api_key_usage_stats()
            .await
            .expect("provider api key stats should rebuild")
            >= 1
    );

    let stats = sqlx::query_as::<_, (i64, i64, f64, Option<i64>)>(
        "SELECT total_requests, total_tokens, total_cost_usd, last_used_at FROM api_keys WHERE id = ?",
    )
    .bind(&api_key_id)
    .fetch_one(&pool)
    .await
    .expect("api key stats should load");
    assert_eq!(stats, (1, 5, 0.5, Some(1_000)));

    let provider_stats = sqlx::query_as::<_, (i64, i64, i64, i64, f64, i64, Option<i64>)>(
        "SELECT request_count, success_count, error_count, total_tokens, total_cost_usd, total_response_time_ms, last_used_at FROM provider_api_keys WHERE id = ?",
    )
    .bind(&provider_key_id)
    .fetch_one(&pool)
    .await
    .expect("provider key stats should load");
    assert_eq!(provider_stats, (1, 1, 0, 5, 0.5, 42, Some(1_000)));
    let model_usage_count: i64 =
        sqlx::query_scalar("SELECT usage_count FROM global_models WHERE name = ?")
            .bind(&model_name)
            .fetch_one(&pool)
            .await
            .expect("global model usage count should load");
    assert_eq!(model_usage_count, 1);
}

#[tokio::test]
async fn mysql_canonical_totals_preserve_legacy_total_tokens_only_rows_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "skipping mysql canonical total_tokens test because AETHER_TEST_MYSQL_URL is unset"
        );
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");

    let suffix = unique_suffix();
    let user_id = format!("total-only-user-{suffix}");
    let api_key_id = format!("total-only-api-key-{suffix}");
    let provider_id = format!("total-only-provider-{suffix}");
    let provider_key_id = format!("total-only-provider-key-{suffix}");
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;

    let repository = MysqlUsageWriteRepository::new(pool.clone());
    let mut usage = sample_usage(
        &format!("total-only-request-{suffix}"),
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "completed",
        "pending",
        1_000,
    );
    usage.input_tokens = Some(0);
    usage.output_tokens = Some(0);
    usage.cache_creation_input_tokens = Some(0);
    usage.cache_creation_ephemeral_5m_input_tokens = Some(0);
    usage.cache_creation_ephemeral_1h_input_tokens = Some(0);
    usage.cache_read_input_tokens = Some(0);
    usage.total_tokens = Some(77);

    let request_id = format!("total-only-request-{suffix}");
    usage.request_id.clone_from(&request_id);
    repository
        .upsert(usage)
        .await
        .expect("legacy total-only usage should upsert");
    sqlx::query("DELETE FROM usage_settlement_snapshots WHERE request_id = ?")
        .bind(&request_id)
        .execute(&pool)
        .await
        .expect("legacy fixture must not have a settlement snapshot");
    let stored = repository
        .find_by_request_id(&request_id)
        .await
        .expect("legacy total-only usage should load")
        .expect("legacy total-only usage should exist");
    assert_eq!(stored.total_tokens, 77);

    repository
        .rebuild_api_key_usage_stats()
        .await
        .expect("api key stats should rebuild");
    repository
        .rebuild_provider_api_key_usage_stats()
        .await
        .expect("provider api key stats should rebuild");

    let api_key_total: i64 = sqlx::query_scalar("SELECT total_tokens FROM api_keys WHERE id = ?")
        .bind(&api_key_id)
        .fetch_one(&pool)
        .await
        .expect("api key total should load");
    let provider_key_total: i64 =
        sqlx::query_scalar("SELECT total_tokens FROM provider_api_keys WHERE id = ?")
            .bind(&provider_key_id)
            .fetch_one(&pool)
            .await
            .expect("provider key total should load");
    assert_eq!(api_key_total, 77);
    assert_eq!(provider_key_total, 77);

    let totals = MysqlUsageStorage::new(pool)
        .summarize_usage_totals_by_user_ids(std::slice::from_ref(&user_id))
        .await
        .expect("user usage totals should load");
    assert_eq!(totals.len(), 1);
    assert_eq!(totals[0].total_tokens, 77);
}

#[tokio::test]
async fn mysql_concurrent_same_request_upserts_enqueue_counters_once_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "skipping mysql usage counter concurrency test because AETHER_TEST_MYSQL_URL is unset"
        );
        return;
    };
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");

    let suffix = unique_suffix();
    let user_id = format!("counter-user-{suffix}");
    let api_key_id = format!("counter-api-key-{suffix}");
    let provider_id = format!("counter-provider-{suffix}");
    let provider_key_id = format!("counter-provider-key-{suffix}");
    let model_name = format!("counter-model-{suffix}");
    let request_id = format!("counter-request-{suffix}");
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;
    sqlx::query("INSERT INTO global_models (id, name, created_at, updated_at) VALUES (?, ?, 1, 1)")
        .bind(format!("counter-global-model-{suffix}"))
        .bind(&model_name)
        .execute(&pool)
        .await
        .expect("global model should seed");

    let repository = MysqlUsageWriteRepository::new(pool.clone());
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let repository = repository.clone();
        let barrier = barrier.clone();
        let mut usage = sample_usage(
            &request_id,
            &user_id,
            &api_key_id,
            &provider_id,
            &provider_key_id,
            "completed",
            "pending",
            1_000,
        );
        usage.model.clone_from(&model_name);
        tasks.push(tokio::spawn(async move {
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
        .flush_usage_counter_deltas(1_000)
        .await
        .expect("usage counter deltas should flush");

    let api_key_requests: i64 =
        sqlx::query_scalar("SELECT total_requests FROM api_keys WHERE id = ?")
            .bind(&api_key_id)
            .fetch_one(&pool)
            .await
            .expect("api key counter should load");
    let provider_key_requests: i64 =
        sqlx::query_scalar("SELECT request_count FROM provider_api_keys WHERE id = ?")
            .bind(&provider_key_id)
            .fetch_one(&pool)
            .await
            .expect("provider key counter should load");
    let model_requests: i64 =
        sqlx::query_scalar("SELECT usage_count FROM global_models WHERE name = ?")
            .bind(&model_name)
            .fetch_one(&pool)
            .await
            .expect("model counter should load");
    let outbox_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM usage_counter_deltas WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(&pool)
            .await
            .expect("usage counter outbox should load");
    let routing_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM usage_routing_snapshots WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(&pool)
            .await
            .expect("routing snapshot should count");
    let settlement_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM usage_settlement_snapshots WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(&pool)
            .await
            .expect("settlement snapshot should count");
    assert_eq!(api_key_requests, 1);
    assert_eq!(provider_key_requests, 1);
    assert_eq!(model_requests, 1);
    assert_eq!(outbox_rows, 3);
    assert_eq!(routing_rows, 1);
    assert_eq!(settlement_rows, 1);
}

#[tokio::test]
async fn mysql_usage_http_capture_round_trips_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql usage HTTP capture test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");
    let suffix = unique_suffix();
    let user_id = format!("capture-user-{suffix}");
    let api_key_id = format!("capture-api-key-{suffix}");
    let provider_id = format!("capture-provider-{suffix}");
    let provider_key_id = format!("capture-provider-key-{suffix}");
    let request_id = format!("capture-request-{suffix}");
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;
    let writer = MysqlUsageWriteRepository::new(pool.clone());

    let mut rich = sample_usage(
        &request_id,
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "pending",
        "pending",
        1_000,
    );
    rich.request_headers = Some(serde_json::json!({"x-client": "one"}));
    rich.provider_request_headers = Some(serde_json::json!({"x-provider": "two"}));
    rich.request_body = Some(serde_json::json!({"request": true}));
    rich.provider_request_body = Some(serde_json::json!({"provider": true}));
    rich.request_body_state = Some(UsageBodyCaptureState::Inline);
    rich.provider_request_body_state = Some(UsageBodyCaptureState::Inline);
    let stored = writer
        .upsert(rich)
        .await
        .expect("MySQL canonical capture should upsert");
    assert_eq!(
        stored.request_headers,
        Some(serde_json::json!({"x-client": "one"}))
    );
    assert_eq!(
        stored.request_body,
        Some(serde_json::json!({"request": true}))
    );
    assert_eq!(
        stored.request_body_state,
        Some(UsageBodyCaptureState::Reference)
    );
    assert_eq!(
        stored.request_body_ref.as_deref(),
        Some(format!("usage://request/{request_id}/request_body").as_str())
    );
    let blob_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM usage_body_blobs WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(&pool)
            .await
            .expect("MySQL canonical blobs should count");
    assert_eq!(blob_count, 2);
    let legacy_body: Option<String> =
        sqlx::query_scalar("SELECT CAST(request_body AS CHAR) FROM `usage` WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(&pool)
            .await
            .expect("legacy body should load");
    assert!(legacy_body.is_none());

    let sparse = sample_usage(
        &request_id,
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "streaming",
        "pending",
        1_001,
    );
    let sparse_stored = writer
        .upsert(sparse)
        .await
        .expect("MySQL sparse capture should upsert");
    assert_eq!(sparse_stored.request_headers, stored.request_headers);
    assert_eq!(sparse_stored.request_body, stored.request_body);

    let mut clear = sample_usage(
        &request_id,
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "streaming",
        "pending",
        1_002,
    );
    clear.request_body = Some(serde_json::json!({"residual": true}));
    clear.request_body_state = Some(UsageBodyCaptureState::None);
    let cleared = writer
        .upsert(clear)
        .await
        .expect("MySQL explicit none should clear");
    assert!(cleared.request_body.is_none());
    assert!(cleared.request_body_ref.is_none());
    assert_eq!(
        cleared.request_body_state,
        Some(UsageBodyCaptureState::None)
    );
    assert_eq!(cleared.provider_request_body, stored.provider_request_body);
}

#[tokio::test]
async fn mysql_usage_canonical_snapshots_round_trip_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql usage snapshot test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");
    let suffix = unique_suffix();
    let user_id = format!("snapshot-user-{suffix}");
    let api_key_id = format!("snapshot-api-key-{suffix}");
    let provider_id = format!("snapshot-provider-{suffix}");
    let provider_key_id = format!("snapshot-provider-key-{suffix}");
    let request_id = format!("snapshot-request-{suffix}");
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;
    let writer = MysqlUsageWriteRepository::new(pool.clone());

    let mut rich = sample_usage(
        &request_id,
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "streaming",
        "pending",
        1_000,
    );
    rich.candidate_id = None;
    rich.candidate_index = None;
    rich.key_name = None;
    rich.planner_kind = None;
    rich.route_family = None;
    rich.route_kind = None;
    rich.execution_path = None;
    rich.input_tokens = Some(100);
    rich.output_tokens = Some(20);
    rich.cache_creation_input_tokens = Some(10);
    rich.cache_creation_ephemeral_5m_input_tokens = Some(10);
    rich.cache_read_input_tokens = Some(30);
    rich.request_metadata = Some(serde_json::json!({
        "trace_id": "rich",
        "candidate_id": "candidate-canonical",
        "candidate_index": 2,
        "key_name": "key-canonical",
        "planner_kind": "fallback",
        "route_family": "chat",
        "route_kind": "remote",
        "execution_path": "converted",
        "billing_snapshot_schema_version": "v3",
        "billing_snapshot_status": "resolved",
        "rate_multiplier": 0.5,
        "input_price_per_1m": 1.1,
        "output_price_per_1m": 2.2,
        "billing_dimensions": {
            "input_tokens": 100,
            "effective_input_tokens": 60,
            "output_tokens": 20,
            "cache_creation_tokens": 10,
            "cache_read_tokens": 30,
            "total_input_context": 100
        },
        "settlement_snapshot": {
            "schema_version": "v3",
            "total_cost": 1.25,
            "actual_total_cost": 1.0,
            "pricing_snapshot": {"pricing_source": "catalog"},
            "billing_plan_snapshot": {"rule_id": "rule-1", "rule_version": "7"}
        }
    }));
    let stored = writer
        .upsert(rich)
        .await
        .expect("MySQL canonical snapshots should upsert");
    assert_eq!(stored.routing_candidate_id(), Some("candidate-canonical"));
    assert_eq!(stored.routing_candidate_index(), Some(2));
    assert_eq!(stored.provider_id.as_deref(), Some(provider_id.as_str()));
    assert_eq!(stored.output_tokens, 20);
    assert_eq!(stored.cache_creation_input_tokens, 10);
    assert_eq!(stored.cache_read_input_tokens, 30);
    assert_eq!(stored.total_tokens, 120);
    assert_eq!(stored.total_cost_usd, 1.25);
    assert_eq!(stored.actual_total_cost_usd, 1.0);
    assert_eq!(stored.settlement_rate_multiplier(), Some(0.5));
    assert_eq!(stored.settlement_input_price_per_1m(), Some(1.1));

    let mut sparse = sample_usage(
        &request_id,
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "streaming",
        "pending",
        1_001,
    );
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
        .expect("MySQL sparse snapshots should merge");
    assert_eq!(
        sparse_stored.routing_candidate_id(),
        Some("candidate-canonical")
    );
    assert_eq!(
        sparse_stored.provider_id.as_deref(),
        Some(provider_id.as_str())
    );
    assert_eq!(sparse_stored.output_tokens, 20);
    assert_eq!(sparse_stored.total_tokens, 120);
    assert_eq!(sparse_stored.total_cost_usd, 1.25);
    assert_eq!(sparse_stored.settlement_rate_multiplier(), Some(0.5));

    let mut terminal = sample_usage(
        &request_id,
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "completed",
        "settled",
        1_002,
    );
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
        .expect("MySQL terminal snapshots should replace");
    assert_eq!(terminal_stored.status, "completed");
    assert_eq!(terminal_stored.billing_status, "settled");
    assert!(terminal_stored.candidate_id.is_none());
    assert!(terminal_stored.provider_id.is_none());
    assert_eq!(terminal_stored.total_tokens, 0);
    assert_eq!(terminal_stored.total_cost_usd, 0.0);
    assert_eq!(terminal_stored.settlement_rate_multiplier(), None);

    let mut late = sample_usage(
        &request_id,
        &user_id,
        &api_key_id,
        &provider_id,
        &provider_key_id,
        "pending",
        "pending",
        1_003,
    );
    late.candidate_id = Some("late-candidate".to_string());
    late.request_metadata = Some(serde_json::json!({
        "trace_id": "late",
        "rate_multiplier": 9,
        "settlement_snapshot": {"schema_version": "late", "total_cost": 99}
    }));
    let after_late = writer
        .upsert(late)
        .await
        .expect("late MySQL pending usage should return terminal record");
    assert_eq!(after_late.status, "completed");
    assert_eq!(after_late.billing_status, "settled");
    assert!(after_late.candidate_id.is_none());
    assert_eq!(after_late.total_cost_usd, 0.0);

    let snapshot_counts: (i64, i64) = sqlx::query_as(
        r#"
SELECT
  (SELECT COUNT(*) FROM usage_routing_snapshots WHERE request_id = ?),
  (SELECT COUNT(*) FROM usage_settlement_snapshots WHERE request_id = ?)
"#,
    )
    .bind(&request_id)
    .bind(&request_id)
    .fetch_one(&pool)
    .await
    .expect("MySQL canonical snapshots should count");
    assert_eq!(snapshot_counts, (1, 1));
}

#[tokio::test]
async fn mysql_usage_read_repository_reads_usage_contract_views_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql usage read smoke test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");

    let suffix = unique_suffix();
    let user_id = format!("user-read-{suffix}");
    let api_key_id = format!("api-key-read-{suffix}");
    let provider_id = format!("provider-read-{suffix}");
    let provider_key_id = format!("provider-key-read-{suffix}");
    let first_request_id = format!("request-read-1-{suffix}");
    let second_request_id = format!("request-read-2-{suffix}");
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;

    let writer = MysqlUsageWriteRepository::new(pool.clone());
    writer
        .upsert(sample_usage(
            &first_request_id,
            &user_id,
            &api_key_id,
            &provider_id,
            &provider_key_id,
            "completed",
            "settled",
            1_000,
        ))
        .await
        .expect("usage should upsert");
    writer
        .upsert(sample_usage(
            &second_request_id,
            &user_id,
            &api_key_id,
            &provider_id,
            &provider_key_id,
            "failed",
            "void",
            1_010,
        ))
        .await
        .expect("usage should upsert");

    sqlx::query("UPDATE `usage` SET username = ?, api_key_name = ? WHERE request_id = ?")
        .bind("legacy-user")
        .bind("legacy-key")
        .bind(&first_request_id)
        .execute(&pool)
        .await
        .expect("legacy usage names should update");

    let reader = MysqlUsageStorage::new(pool);
    let records = reader
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(900),
            created_until_unix_secs: Some(1_100),
            user_id: Some(user_id.clone()),
            newest_first: true,
            ..UsageAuditListQuery::default()
        })
        .await
        .expect("usage records should load");
    let loaded = records
        .iter()
        .find(|item| item.request_id == first_request_id)
        .expect("usage should exist");
    assert_eq!(loaded.total_tokens, 5);
    assert_eq!(loaded.billing_status, "settled");
    assert_eq!(loaded.username.as_deref(), Some("legacy-user"));
    assert_eq!(loaded.api_key_name.as_deref(), Some("legacy-key"));
    assert_eq!(
        records
            .iter()
            .filter(|item| item.user_id.as_deref() == Some(&user_id))
            .count(),
        2
    );
}

#[tokio::test]
async fn mysql_usage_cleanup_executes_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql usage cleanup test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");

    let suffix = unique_suffix();
    let user_id = format!("cleanup-user-{suffix}");
    let api_key_id = format!("cleanup-api-key-{suffix}");
    let provider_id = format!("cleanup-provider-{suffix}");
    let provider_key_id = format!("cleanup-provider-key-{suffix}");
    let request_id = format!("cleanup-request-{suffix}");
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;
    let repository = MysqlUsageWriteRepository::new(pool.clone());
    repository
        .upsert(sample_usage(
            &request_id,
            &user_id,
            &api_key_id,
            &provider_id,
            &provider_key_id,
            "completed",
            "settled",
            10,
        ))
        .await
        .expect("cleanup usage should seed");
    sqlx::query(
        "UPDATE `usage` SET request_headers = '{\"keep\":true}', request_body = '{\"body\":true}' WHERE request_id = ?",
    )
    .bind(&request_id)
    .execute(&pool)
    .await
    .expect("cleanup fields should seed");

    let window = UsageCleanupWindow {
        detail_cutoff: DateTime::from_timestamp(20, 0).expect("valid detail cutoff"),
        compressed_cutoff: DateTime::from_timestamp(5, 0).expect("valid compressed cutoff"),
        header_cutoff: DateTime::from_timestamp(20, 0).expect("valid header cutoff"),
        log_cutoff: DateTime::from_timestamp(5, 0).expect("valid log cutoff"),
    };
    let detail_only = UsageCleanupTargets {
        detail_body: true,
        compressed_body: false,
        headers: false,
        records: false,
        expired_keys: false,
    };
    let preview = repository
        .preview_usage_cleanup(&window, detail_only, UsageCleanupExecutionMode::Policy)
        .await
        .expect("MySQL cleanup preview should load");
    assert!(preview.detail >= 1);
    let summary = repository
        .cleanup_usage(
            &window,
            1,
            false,
            detail_only,
            UsageCleanupExecutionMode::Policy,
        )
        .await
        .expect("MySQL detail cleanup should succeed");
    assert!(summary.body_externalized >= 1);
    let body_ref: String =
        sqlx::query_scalar("SELECT request_body_ref FROM usage_http_audits WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(&pool)
            .await
            .expect("externalized body ref should load");
    assert_eq!(
        body_ref,
        format!("usage://request/{request_id}/request_body")
    );

    let headers_only = UsageCleanupTargets {
        detail_body: false,
        compressed_body: false,
        headers: true,
        records: false,
        expired_keys: false,
    };
    let summary = repository
        .cleanup_usage(
            &window,
            1,
            false,
            headers_only,
            UsageCleanupExecutionMode::Policy,
        )
        .await
        .expect("MySQL header cleanup should succeed");
    assert!(summary.header_cleaned >= 1);
    let request_headers: Option<String> =
        sqlx::query_scalar("SELECT request_headers FROM `usage` WHERE request_id = ?")
            .bind(&request_id)
            .fetch_one(&pool)
            .await
            .expect("cleaned headers should load");
    assert!(request_headers.is_none());

    let before_now_request_id = format!("cleanup-before-now-{suffix}");
    repository
        .upsert(sample_usage(
            &before_now_request_id,
            &user_id,
            &api_key_id,
            &provider_id,
            &provider_key_id,
            "completed",
            "settled",
            10,
        ))
        .await
        .expect("before-now usage should seed");
    sqlx::query(
        "UPDATE `usage` SET request_body = '{\"raw\":true}', request_body_compressed = ? WHERE request_id = ?",
    )
    .bind(vec![0x1f_u8, 0x8b])
    .bind(&before_now_request_id)
    .execute(&pool)
    .await
    .expect("before-now fields should seed");
    let summary = repository
        .cleanup_usage(
            &window,
            1,
            false,
            UsageCleanupTargets::body_targets(),
            UsageCleanupExecutionMode::BeforeNowBodyFields,
        )
        .await
        .expect("MySQL before-now cleanup should succeed");
    assert!(summary.body_externalized >= 1);
    assert!(summary.body_cleaned >= 1);
    let body_fields: (Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT CAST(request_body AS CHAR), request_body_compressed FROM `usage` WHERE request_id = ?",
    )
    .bind(&before_now_request_id)
    .fetch_one(&pool)
    .await
    .expect("before-now fields should load");
    assert_eq!(body_fields, (None, None));
}

async fn seed_stats_targets(
    pool: &sqlx::MySqlPool,
    user_id: &str,
    api_key_id: &str,
    provider_id: &str,
    provider_key_id: &str,
) {
    sqlx::query(
        r#"
INSERT INTO users (id, auth_source, created_at, updated_at)
VALUES (?, 'local', 1, 1)
"#,
    )
    .bind(user_id)
    .execute(pool)
    .await
    .expect("user should seed");

    sqlx::query(
        r#"
INSERT INTO api_keys (id, user_id, key_hash, created_at, updated_at)
VALUES (?, ?, ?, 1, 1)
"#,
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(format!("hash-{api_key_id}"))
    .execute(pool)
    .await
    .expect("api key should seed");

    sqlx::query(
        r#"
INSERT INTO providers (id, name, provider_type, created_at, updated_at)
VALUES (?, ?, 'openai', 1, 1)
"#,
    )
    .bind(provider_id)
    .bind(format!("Provider {provider_id}"))
    .execute(pool)
    .await
    .expect("provider should seed");

    sqlx::query(
        r#"
INSERT INTO provider_api_keys (id, provider_id, name, created_at, updated_at)
VALUES (?, ?, ?, 1, 1)
"#,
    )
    .bind(provider_key_id)
    .bind(provider_id)
    .bind(format!("Provider Key {provider_key_id}"))
    .execute(pool)
    .await
    .expect("provider key should seed");
}

#[allow(clippy::too_many_arguments)]
fn sample_usage(
    request_id: &str,
    user_id: &str,
    api_key_id: &str,
    provider_id: &str,
    provider_key_id: &str,
    status: &str,
    billing_status: &str,
    updated_at: u64,
) -> UpsertUsageRecord {
    UpsertUsageRecord {
        request_id: request_id.to_string(),
        user_id: Some(user_id.to_string()),
        api_key_id: Some(api_key_id.to_string()),
        username: Some("legacy-user".to_string()),
        api_key_name: Some("legacy-key".to_string()),
        provider_name: "Provider One".to_string(),
        model: "model-1".to_string(),
        target_model: Some("target-model".to_string()),
        provider_id: Some(provider_id.to_string()),
        provider_endpoint_id: Some("endpoint-1".to_string()),
        provider_api_key_id: Some(provider_key_id.to_string()),
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

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}
