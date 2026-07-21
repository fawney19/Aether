use super::{MysqlUsageStorage, MysqlUsageWriteRepository};
use crate::run_migrations;
use aether_data_contracts::repository::usage::{UpsertUsageRecord, UsageWriteRepository};

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
async fn mysql_usage_write_repository_upserts_when_url_is_set() {
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
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;

    let repository = MysqlUsageWriteRepository::new(pool.clone());
    let record = repository
        .upsert(sample_usage(
            &format!("request-{suffix}"),
            &user_id,
            &api_key_id,
            &provider_id,
            &provider_key_id,
            "completed",
            "pending",
            1_000,
        ))
        .await
        .expect("usage should upsert");

    assert_eq!(record.api_key_id.as_deref(), Some(api_key_id.as_str()));
    assert_eq!(
        record.provider_api_key_id.as_deref(),
        Some(provider_key_id.as_str())
    );
    assert_eq!(record.total_tokens, 7);
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

    let stats = sqlx::query_as::<_, (i64, i64, f64, Option<i64>)>(
        "SELECT total_requests, total_tokens, total_cost_usd, last_used_at FROM api_keys WHERE id = ?",
    )
    .bind(&api_key_id)
    .fetch_one(&pool)
    .await
    .expect("api key stats should load");
    assert_eq!(stats, (1, 7, 0.5, Some(1_000)));

    let provider_stats = sqlx::query_as::<_, (i64, i64, i64, i64, f64, i64, Option<i64>)>(
        "SELECT request_count, success_count, error_count, total_tokens, total_cost_usd, total_response_time_ms, last_used_at FROM provider_api_keys WHERE id = ?",
    )
    .bind(&provider_key_id)
    .fetch_one(&pool)
    .await
    .expect("provider key stats should load");
    assert_eq!(provider_stats, (1, 1, 0, 7, 0.5, 42, Some(1_000)));
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
    seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;

    let writer = MysqlUsageWriteRepository::new(pool.clone());
    writer
        .upsert(sample_usage(
            &format!("request-read-1-{suffix}"),
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
            &format!("request-read-2-{suffix}"),
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

    let reader = MysqlUsageStorage::new(pool);
    let records = reader
        .load_usage_records()
        .await
        .expect("usage records should load");
    let loaded = records
        .iter()
        .find(|item| item.request_id == format!("request-read-1-{suffix}"))
        .expect("usage should exist");
    assert_eq!(loaded.total_tokens, 7);
    assert_eq!(loaded.billing_status, "settled");
    assert_eq!(
        records
            .iter()
            .filter(|item| item.user_id.as_deref() == Some(&user_id))
            .count(),
        2
    );
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
