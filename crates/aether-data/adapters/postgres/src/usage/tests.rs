use chrono::{TimeZone, Utc};
use serde_json::json;
use sqlx::Row;

use super::{
    attach_compressed_body_refs, attach_usage_http_audit_body_refs,
    attach_usage_routing_snapshot_metadata, attach_usage_settlement_pricing_snapshot_metadata,
    clear_previous_request_body_facts, inflate_usage_json_value,
    prepare_request_metadata_for_body_storage, prepare_usage_body_storage,
    prepare_usage_upsert_context, request_body_capture_replaces_derived_facts,
    resolved_read_usage_body_ref, resolved_write_usage_body_ref,
    split_dashboard_daily_aggregate_range, split_dashboard_hourly_aggregate_range,
    usage_body_capture_state_for_storage, usage_body_ref, usage_capture_update_allowed,
    usage_effective_input_tokens, usage_http_audit_body_refs, usage_http_audit_capture_mode,
    usage_routing_snapshot_from_usage, usage_settlement_pricing_snapshot_from_usage,
    usage_total_input_context, AggregateRangeSplit, SqlxUsageReadRepository, UsageHttpAuditRefs,
    UsageRoutingSnapshot, UsageSettlementPricingSnapshot, MAX_INLINE_USAGE_BODY_BYTES,
};
use crate::{PostgresPoolConfig, PostgresPoolFactory};
use aether_data_contracts::repository::usage::{
    UpsertUsageRecord, UsageAuditListQuery, UsageBodyCaptureState, UsageBodyField,
    UsageCostSavingsSummaryQuery, UsageDashboardDailyBreakdownQuery, UsageDashboardSummaryQuery,
    UsageProviderPerformanceQuery, UsageTimeSeriesGranularity,
};

fn fast_clear_usage_record(
    request_id: &str,
    provider_name: &str,
    now_unix_secs: u64,
    terminal: bool,
    terminal_state: UsageBodyCaptureState,
    terminal_service_tier: Option<&str>,
) -> UpsertUsageRecord {
    UpsertUsageRecord {
        request_id: request_id.to_string(),
        user_id: None,
        api_key_id: None,
        username: None,
        api_key_name: None,
        provider_name: provider_name.to_string(),
        model: "gpt-5".to_string(),
        target_model: Some("gpt-5".to_string()),
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
        input_tokens: terminal.then_some(1),
        output_tokens: terminal.then_some(1),
        total_tokens: terminal.then_some(2),
        cache_creation_input_tokens: None,
        cache_creation_ephemeral_5m_input_tokens: None,
        cache_creation_ephemeral_1h_input_tokens: None,
        cache_read_input_tokens: None,
        cache_creation_cost_usd: None,
        cache_read_cost_usd: None,
        output_price_per_1m: None,
        total_cost_usd: None,
        actual_total_cost_usd: None,
        status_code: terminal.then_some(200),
        error_message: None,
        error_category: None,
        response_time_ms: terminal.then_some(10),
        first_byte_time_ms: None,
        status: if terminal { "completed" } else { "pending" }.to_string(),
        billing_status: "pending".to_string(),
        request_headers: None,
        request_body: None,
        request_body_ref: None,
        request_body_state: None,
        provider_request_headers: None,
        provider_request_body: (!terminal).then(|| {
            json!({
                "model": "gpt-5",
                "service_tier": "priority"
            })
        }),
        provider_request_body_ref: None,
        provider_request_body_state: Some(if terminal {
            terminal_state
        } else {
            UsageBodyCaptureState::Inline
        }),
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
        request_metadata: if terminal {
            terminal_service_tier.map(|tier| json!({"provider_service_tier": tier}))
        } else {
            Some(json!({"provider_service_tier": "priority"}))
        },
        finalized_at_unix_secs: terminal.then_some(now_unix_secs + 1),
        created_at_unix_ms: Some(now_unix_secs),
        updated_at_unix_secs: now_unix_secs + u64::from(terminal),
    }
}

#[tokio::test]
#[ignore = "requires AETHER_TEST_DATABASE_URL and PostgreSQL migrations"]
async fn live_terminal_none_capture_clears_fast_from_detail_and_lightweight_lists() {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("AETHER_TEST_DATABASE_URL must point at the test database");
    let factory = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url,
        min_connections: 1,
        max_connections: 2,
        acquire_timeout_ms: 10_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("factory should build");
    let repository =
        SqlxUsageReadRepository::new(factory.connect_lazy().expect("lazy pool should build"));
    crate::run_migrations(repository.pool())
        .await
        .expect("test database migrations should succeed");

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let request_id = format!("req-fast-clear-{suffix}");
    let provider_name = format!("fast-clear-{suffix}");
    let now_unix_secs = Utc::now().timestamp().max(0) as u64;

    let mut pending_record = fast_clear_usage_record(
        &request_id,
        &provider_name,
        now_unix_secs,
        false,
        UsageBodyCaptureState::Inline,
        Some("priority"),
    );
    pending_record.candidate_id = Some("candidate-a".to_string());
    pending_record.candidate_index = Some(1);
    pending_record.key_name = Some("key-a".to_string());
    pending_record.planner_kind = Some("planner-a".to_string());
    pending_record.route_family = Some("route-family-a".to_string());
    pending_record.route_kind = Some("route-kind-a".to_string());
    pending_record.execution_path = Some("path-a".to_string());
    pending_record.local_execution_runtime_miss_reason = Some("miss-a".to_string());
    pending_record.request_metadata = Some(json!({
        "trace_id": "trace-a",
        "provider_service_tier": "priority",
        "provider_actual_service_tier": "priority",
        "billing_snapshot": {
            "schema_version": "2.0",
            "status": "complete",
            "resolved_variables": {
                "input_price_per_1m": 30.0,
                "output_price_per_1m": 150.0
            }
        },
        "settlement_snapshot": {
            "schema_version": "2.0",
            "pricing_snapshot": {
                "pricing_source": "processing_tier",
                "service_tier": "priority"
            },
            "billing_plan_snapshot": {
                "rule_id": "fast-rule",
                "rule_version": "1"
            }
        },
        "billing_dimensions": {"service_tier": "priority"},
        "rate_multiplier": 2.0,
        "input_price_per_1m": 30.0,
        "output_price_per_1m": 150.0
    }));
    let pending = repository
        .upsert(pending_record)
        .await
        .expect("pending usage should persist");
    assert_eq!(pending.provider_service_tier().as_deref(), Some("priority"));

    let mut terminal_record = fast_clear_usage_record(
        &request_id,
        &provider_name,
        now_unix_secs,
        true,
        UsageBodyCaptureState::None,
        None,
    );
    terminal_record.provider_id = Some("final-provider-id".to_string());
    terminal_record.provider_endpoint_id = Some("final-endpoint-id".to_string());
    terminal_record.provider_api_key_id = Some("final-key-id".to_string());
    terminal_record.target_model = None;
    let terminal = repository
        .upsert(terminal_record)
        .await
        .expect("terminal usage should persist");
    assert_eq!(terminal.provider_service_tier(), None);

    let stored = repository
        .find_by_request_id(&request_id)
        .await
        .expect("detail lookup should succeed")
        .expect("usage should exist");
    assert_eq!(
        stored.provider_request_body_state,
        Some(UsageBodyCaptureState::None)
    );
    assert_eq!(stored.provider_service_tier(), None);
    assert_eq!(stored.provider_actual_service_tier(), None);
    let stored_metadata = stored
        .request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .expect("terminal audit metadata should remain");
    assert_eq!(
        stored_metadata
            .get("trace_id")
            .and_then(serde_json::Value::as_str),
        Some("trace-a")
    );
    for stale_key in [
        "provider_actual_service_tier",
        "billing_snapshot",
        "settlement_snapshot",
        "billing_dimensions",
        "rate_multiplier",
        "input_price_per_1m",
        "output_price_per_1m",
        "candidate_id",
    ] {
        assert!(
            stored_metadata.get(stale_key).is_none(),
            "terminal metadata retained stale key {stale_key}"
        );
    }
    assert_eq!(stored.settlement_rate_multiplier(), None);
    assert_eq!(stored.settlement_input_price_per_1m(), None);
    assert_eq!(stored.settlement_output_price_per_1m(), None);

    let settlement_row = sqlx::query(
        "SELECT settlement_snapshot, billing_dimensions, CAST(rate_multiplier AS DOUBLE PRECISION) AS rate_multiplier, CAST(input_price_per_1m AS DOUBLE PRECISION) AS input_price_per_1m, CAST(output_price_per_1m AS DOUBLE PRECISION) AS output_price_per_1m FROM usage_settlement_snapshots WHERE request_id = $1",
    )
    .bind(&request_id)
    .fetch_one(repository.pool())
    .await
    .expect("settlement snapshot row should be readable");
    assert!(settlement_row
        .try_get::<Option<serde_json::Value>, _>("settlement_snapshot")
        .expect("settlement snapshot should decode")
        .is_none());
    assert!(settlement_row
        .try_get::<Option<serde_json::Value>, _>("billing_dimensions")
        .expect("billing dimensions should decode")
        .is_none());
    assert!(settlement_row
        .try_get::<Option<f64>, _>("rate_multiplier")
        .expect("rate multiplier should decode")
        .is_none());
    assert!(settlement_row
        .try_get::<Option<f64>, _>("input_price_per_1m")
        .expect("input price should decode")
        .is_none());
    assert!(settlement_row
        .try_get::<Option<f64>, _>("output_price_per_1m")
        .expect("output price should decode")
        .is_none());

    let physical_metadata = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT request_metadata FROM \"usage\" WHERE request_id = $1",
    )
    .bind(&request_id)
    .fetch_one(repository.pool())
    .await
    .expect("physical metadata should be readable")
    .expect("clear tombstone should be stored");
    assert!(physical_metadata.get("provider_service_tier").is_none());

    let physical_body = sqlx::query(
        "SELECT provider_request_body, provider_request_body_compressed FROM \"usage\" WHERE request_id = $1",
    )
    .bind(&request_id)
    .fetch_one(repository.pool())
    .await
    .expect("physical body columns should be readable");
    assert!(physical_body
        .try_get::<Option<serde_json::Value>, _>("provider_request_body")
        .expect("provider body column should decode")
        .is_none());
    assert!(physical_body
        .try_get::<Option<Vec<u8>>, _>("provider_request_body_compressed")
        .expect("compressed provider body column should decode")
        .is_none());

    let physical_http = sqlx::query(
        "SELECT provider_request_body_ref, provider_request_body_state FROM usage_http_audits WHERE request_id = $1",
    )
    .bind(&request_id)
    .fetch_one(repository.pool())
    .await
    .expect("HTTP audit row should be readable");
    assert!(physical_http
        .try_get::<Option<String>, _>("provider_request_body_ref")
        .expect("provider body ref should decode")
        .is_none());
    assert_eq!(
        physical_http
            .try_get::<Option<String>, _>("provider_request_body_state")
            .expect("provider body state should decode")
            .as_deref(),
        Some("none")
    );
    let physical_blob_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM usage_body_blobs WHERE request_id = $1 AND body_field = 'provider_request_body'",
    )
    .bind(&request_id)
    .fetch_one(repository.pool())
    .await
    .expect("provider body blob count should be readable");
    assert_eq!(physical_blob_count, 0);

    // A late pending candidate must not resurrect the terminal capture, metadata, or detached
    // blob after the final state has been written.
    let mut late_record = fast_clear_usage_record(
        &request_id,
        "late-provider",
        now_unix_secs + 2,
        false,
        UsageBodyCaptureState::Inline,
        Some("priority"),
    );
    late_record.model = "late-model".to_string();
    late_record.target_model = Some("late-target".to_string());
    late_record.provider_id = Some("late-provider-id".to_string());
    late_record.provider_endpoint_id = Some("late-endpoint-id".to_string());
    late_record.provider_api_key_id = Some("late-key-id".to_string());
    late_record.endpoint_api_format = Some("late:format".to_string());
    late_record.candidate_id = Some("late-candidate".to_string());
    let late = repository
        .upsert(late_record)
        .await
        .expect("late pending usage should be accepted without regressing capture");
    assert_eq!(late.provider_service_tier(), None);
    assert_eq!(late.provider_name, provider_name);
    assert_eq!(late.model, "gpt-5");
    assert_eq!(late.target_model, None);
    assert_eq!(late.provider_id.as_deref(), Some("final-provider-id"));
    assert_eq!(
        late.provider_endpoint_id.as_deref(),
        Some("final-endpoint-id")
    );
    assert_eq!(late.provider_api_key_id.as_deref(), Some("final-key-id"));
    assert_eq!(late.endpoint_api_format.as_deref(), Some("openai:chat"));
    assert_eq!(late.candidate_id, None);
    let late_blob_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM usage_body_blobs WHERE request_id = $1 AND body_field = 'provider_request_body'",
    )
    .bind(&request_id)
    .fetch_one(repository.pool())
    .await
    .expect("late provider body blob count should be readable");
    assert_eq!(late_blob_count, 0);

    let list_item = repository
        .list_usage_audits(&UsageAuditListQuery {
            provider_name: Some(provider_name.clone()),
            limit: Some(10),
            newest_first: true,
            ..UsageAuditListQuery::default()
        })
        .await
        .expect("usage list should succeed")
        .into_iter()
        .find(|item| item.request_id == request_id)
        .expect("usage list should contain the test row");
    assert_eq!(list_item.provider_request_body_state, None);
    assert_eq!(list_item.provider_service_tier(), None);
    assert_eq!(list_item.target_model, None);
    assert_eq!(list_item.candidate_id, None);
    assert_eq!(list_item.candidate_index, None);
    assert_eq!(list_item.key_name, None);
    assert_eq!(list_item.planner_kind, None);
    assert_eq!(list_item.route_family, None);
    assert_eq!(list_item.route_kind, None);
    assert_eq!(list_item.execution_path, None);
    assert_eq!(list_item.local_execution_runtime_miss_reason, None);

    let recent_item = repository
        .list_recent_usage_audits(None, 20)
        .await
        .expect("recent usage list should succeed")
        .into_iter()
        .find(|item| item.request_id == request_id)
        .expect("recent usage list should contain the test row");
    assert_eq!(recent_item.provider_request_body_state, None);
    assert_eq!(recent_item.provider_service_tier(), None);

    sqlx::query("DELETE FROM \"usage\" WHERE request_id = $1")
        .bind(&request_id)
        .execute(repository.pool())
        .await
        .expect("test usage should be removed");

    for (index, state, incoming_tier, expected_tier) in [
        ("disabled", UsageBodyCaptureState::Disabled, None, None),
        ("truncated", UsageBodyCaptureState::Truncated, None, None),
        (
            "unavailable",
            UsageBodyCaptureState::Unavailable,
            None,
            None,
        ),
        (
            "none-stale-metadata",
            UsageBodyCaptureState::None,
            Some("priority"),
            None,
        ),
        (
            "disabled-preserved",
            UsageBodyCaptureState::Disabled,
            Some("priority"),
            Some("priority"),
        ),
        (
            "truncated-preserved",
            UsageBodyCaptureState::Truncated,
            Some("priority"),
            Some("priority"),
        ),
    ] {
        let request_id = format!("req-fast-clear-{suffix}-{index}");
        let provider_name = format!("fast-clear-{suffix}-{index}");
        repository
            .upsert(fast_clear_usage_record(
                &request_id,
                &provider_name,
                now_unix_secs,
                false,
                UsageBodyCaptureState::Inline,
                Some("priority"),
            ))
            .await
            .expect("typed-state pending usage should persist");
        let terminal = repository
            .upsert(fast_clear_usage_record(
                &request_id,
                &provider_name,
                now_unix_secs + 1,
                true,
                state,
                incoming_tier,
            ))
            .await
            .expect("typed-state terminal usage should persist");
        assert_eq!(
            terminal.provider_service_tier().as_deref(),
            expected_tier,
            "state={state:?} should use only same-source metadata"
        );
        let list_item = repository
            .list_usage_audits(&UsageAuditListQuery {
                provider_name: Some(provider_name.clone()),
                limit: Some(10),
                newest_first: true,
                ..UsageAuditListQuery::default()
            })
            .await
            .expect("typed-state list should succeed")
            .into_iter()
            .find(|item| item.request_id == request_id)
            .expect("typed-state row should be listed");
        assert_eq!(list_item.provider_service_tier().as_deref(), expected_tier);
        sqlx::query("DELETE FROM \"usage\" WHERE request_id = $1")
            .bind(&request_id)
            .execute(repository.pool())
            .await
            .expect("typed-state test usage should be removed");
    }
}

#[tokio::test]
#[ignore = "requires AETHER_TEST_DATABASE_URL and a populated PostgreSQL database"]
async fn live_dashboard_combined_summary_matches_separate_queries() {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("AETHER_TEST_DATABASE_URL must point at the test database");
    let factory = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url,
        min_connections: 1,
        max_connections: 4,
        acquire_timeout_ms: 10_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("factory should build");
    let repository =
        SqlxUsageReadRepository::new(factory.connect_lazy().expect("lazy pool should build"));
    let until = Utc::now().timestamp().max(0) as u64;
    let query = UsageDashboardSummaryQuery {
        created_from_unix_secs: until.saturating_sub(7 * 24 * 60 * 60),
        created_until_unix_secs: until,
        user_id: None,
    };

    let combined_started = std::time::Instant::now();
    let combined = repository
        .summarize_dashboard_stats(&query)
        .await
        .expect("combined dashboard summary should succeed");
    let combined_elapsed = combined_started.elapsed();
    let separate_started = std::time::Instant::now();
    let usage = repository
        .summarize_dashboard_usage(&query)
        .await
        .expect("standalone usage summary should succeed");
    let cost_savings = repository
        .summarize_usage_cost_savings(&UsageCostSavingsSummaryQuery {
            created_from_unix_secs: query.created_from_unix_secs,
            created_until_unix_secs: query.created_until_unix_secs,
            user_id: None,
            provider_name: None,
            model: None,
        })
        .await
        .expect("standalone cost-savings summary should succeed");
    let separate_elapsed = separate_started.elapsed();

    eprintln!("combined={combined_elapsed:?} separate={separate_elapsed:?}",);

    assert_eq!(combined.usage, usage);
    assert_eq!(
        combined.cost_savings.cache_read_tokens,
        cost_savings.cache_read_tokens
    );
    assert!(
        (combined.cost_savings.cache_read_cost_usd - cost_savings.cache_read_cost_usd).abs() < 1e-9
    );
    assert!(
        (combined.cost_savings.cache_creation_cost_usd - cost_savings.cache_creation_cost_usd)
            .abs()
            < 1e-9
    );
    assert!(
        (combined.cost_savings.estimated_full_cost_usd - cost_savings.estimated_full_cost_usd)
            .abs()
            < 1e-9
    );
}

#[tokio::test]
#[ignore = "requires AETHER_TEST_DATABASE_URL and a populated PostgreSQL database"]
async fn live_provider_performance_grouping_sets_matches_separate_queries() {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("AETHER_TEST_DATABASE_URL must point at the test database");
    let factory = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url,
        min_connections: 1,
        max_connections: 4,
        acquire_timeout_ms: 10_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("factory should build");
    let repository =
        SqlxUsageReadRepository::new(factory.connect_lazy().expect("lazy pool should build"));
    let until = Utc::now().timestamp().max(0) as u64;
    let query = UsageProviderPerformanceQuery {
        created_from_unix_secs: until.saturating_sub(24 * 60 * 60),
        created_until_unix_secs: until,
        granularity: UsageTimeSeriesGranularity::Hour,
        tz_offset_minutes: 480,
        limit: 8,
        provider_id: None,
        model: None,
        api_format: None,
        endpoint_kind: None,
        is_stream: None,
        has_format_conversion: None,
        slow_threshold_ms: 10_000,
        include_timeline: false,
    };

    let expected_summary = repository
        .summarize_usage_provider_performance_summary(&query)
        .await
        .expect("standalone provider summary should succeed");
    let (_, expected_providers) = repository
        .summarize_usage_provider_performance_groups(&query, false)
        .await
        .expect("standalone provider ranking should succeed");
    let actual = repository
        .summarize_usage_provider_performance(&query)
        .await
        .expect("combined provider performance should succeed");

    assert_eq!(actual.summary.request_count, expected_summary.request_count);
    assert_eq!(actual.summary.success_count, expected_summary.success_count);
    assert_eq!(
        actual.summary.p90_response_time_ms,
        expected_summary.p90_response_time_ms
    );
    assert_eq!(
        actual.summary.p99_response_time_ms,
        expected_summary.p99_response_time_ms
    );
    assert_eq!(
        actual.summary.p90_first_byte_time_ms,
        expected_summary.p90_first_byte_time_ms
    );
    assert_eq!(
        actual.summary.p99_first_byte_time_ms,
        expected_summary.p99_first_byte_time_ms
    );
    assert_eq!(
        actual.summary.tps_sample_count,
        expected_summary.tps_sample_count
    );
    assert_eq!(
        actual.summary.response_time_sample_count,
        expected_summary.response_time_sample_count
    );
    assert_eq!(
        actual.summary.first_byte_sample_count,
        expected_summary.first_byte_sample_count
    );
    assert_eq!(
        actual.summary.slow_request_count,
        expected_summary.slow_request_count
    );
    let assert_optional_f64_close = |actual_value: Option<f64>, expected_value: Option<f64>| match (
        actual_value,
        expected_value,
    ) {
        (Some(actual_value), Some(expected_value)) => {
            assert!((actual_value - expected_value).abs() < 1e-9)
        }
        (None, None) => {}
        values => panic!("average presence differs: {values:?}"),
    };
    for (actual_value, expected_value) in [
        (
            actual.summary.avg_output_tps,
            expected_summary.avg_output_tps,
        ),
        (
            actual.summary.avg_first_byte_time_ms,
            expected_summary.avg_first_byte_time_ms,
        ),
        (
            actual.summary.avg_response_time_ms,
            expected_summary.avg_response_time_ms,
        ),
    ] {
        assert_optional_f64_close(actual_value, expected_value);
    }

    assert_eq!(
        actual.providers.len(),
        expected_providers.len().min(query.limit)
    );
    for (actual_provider, expected_provider) in actual.providers.iter().zip(&expected_providers) {
        assert_eq!(actual_provider.provider_id, expected_provider.provider_id);
        assert_eq!(actual_provider.provider, expected_provider.provider);
        assert_eq!(
            actual_provider.request_count,
            expected_provider.request_count
        );
        assert_eq!(
            actual_provider.success_count,
            expected_provider.success_count
        );
        assert_eq!(
            actual_provider.output_tokens,
            expected_provider.output_tokens
        );
        assert_eq!(
            actual_provider.p90_response_time_ms,
            expected_provider.p90_response_time_ms
        );
        assert_eq!(
            actual_provider.p99_response_time_ms,
            expected_provider.p99_response_time_ms
        );
        assert_eq!(
            actual_provider.p90_first_byte_time_ms,
            expected_provider.p90_first_byte_time_ms
        );
        assert_eq!(
            actual_provider.p99_first_byte_time_ms,
            expected_provider.p99_first_byte_time_ms
        );
        assert_eq!(
            actual_provider.tps_sample_count,
            expected_provider.tps_sample_count
        );
        assert_eq!(
            actual_provider.response_time_sample_count,
            expected_provider.response_time_sample_count
        );
        assert_eq!(
            actual_provider.first_byte_sample_count,
            expected_provider.first_byte_sample_count
        );
        assert_eq!(
            actual_provider.slow_request_count,
            expected_provider.slow_request_count
        );
        for (actual_value, expected_value) in [
            (
                actual_provider.avg_output_tps,
                expected_provider.avg_output_tps,
            ),
            (
                actual_provider.avg_first_byte_time_ms,
                expected_provider.avg_first_byte_time_ms,
            ),
            (
                actual_provider.avg_response_time_ms,
                expected_provider.avg_response_time_ms,
            ),
        ] {
            assert_optional_f64_close(actual_value, expected_value);
        }
    }
    assert!(actual.timeline.is_empty());
}

#[tokio::test]
#[ignore = "requires AETHER_TEST_DATABASE_URL and a populated PostgreSQL database"]
async fn live_dashboard_daily_breakdown_uses_canonical_covering_read_path() {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("AETHER_TEST_DATABASE_URL must point at the test database");
    let factory = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url,
        min_connections: 1,
        max_connections: 2,
        acquire_timeout_ms: 10_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("factory should build");
    let repository =
        SqlxUsageReadRepository::new(factory.connect_lazy().expect("lazy pool should build"));
    let until = Utc::now().timestamp().max(0) as u64;
    let started = std::time::Instant::now();
    let rows = repository
        .list_dashboard_daily_breakdown(&UsageDashboardDailyBreakdownQuery {
            created_from_unix_secs: until.saturating_sub(7 * 24 * 60 * 60),
            created_until_unix_secs: until,
            tz_offset_minutes: 480,
            user_id: None,
        })
        .await
        .expect("daily breakdown should succeed");
    eprintln!(
        "daily_breakdown={:?} rows={}",
        started.elapsed(),
        rows.len()
    );
    assert!(!rows.is_empty());
}

#[tokio::test]
#[ignore = "requires AETHER_TEST_DATABASE_URL and a populated PostgreSQL database"]
async fn live_dashboard_admin_hot_path_uses_two_parallel_scans_instead_of_four_serial_scans() {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("AETHER_TEST_DATABASE_URL must point at the test database");
    let factory = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url,
        min_connections: 2,
        max_connections: 4,
        acquire_timeout_ms: 10_000,
        idle_timeout_ms: 30_000,
        max_lifetime_ms: 60_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("factory should build");
    let repository =
        SqlxUsageReadRepository::new(factory.connect_lazy().expect("lazy pool should build"));
    let until = Utc::now().timestamp().max(0) as u64;
    let period = UsageDashboardSummaryQuery {
        created_from_unix_secs: until.saturating_sub(30 * 24 * 60 * 60),
        created_until_unix_secs: until,
        user_id: None,
    };
    let today = UsageDashboardSummaryQuery {
        created_from_unix_secs: until.saturating_sub(24 * 60 * 60),
        created_until_unix_secs: until,
        user_id: None,
    };

    let _ = tokio::join!(
        repository.summarize_dashboard_stats(&period),
        repository.summarize_dashboard_stats(&today),
    );
    let optimized_started = std::time::Instant::now();
    let (period_combined, today_combined) = tokio::join!(
        repository.summarize_dashboard_stats(&period),
        repository.summarize_dashboard_stats(&today),
    );
    let optimized_elapsed = optimized_started.elapsed();
    let period_combined = period_combined.expect("period combined summary should succeed");
    let today_combined = today_combined.expect("today combined summary should succeed");

    let legacy_started = std::time::Instant::now();
    let period_usage = repository
        .summarize_dashboard_usage(&period)
        .await
        .expect("period usage summary should succeed");
    let today_usage = repository
        .summarize_dashboard_usage(&today)
        .await
        .expect("today usage summary should succeed");
    let today_savings = repository
        .summarize_usage_cost_savings(&UsageCostSavingsSummaryQuery {
            created_from_unix_secs: today.created_from_unix_secs,
            created_until_unix_secs: today.created_until_unix_secs,
            user_id: None,
            provider_name: None,
            model: None,
        })
        .await
        .expect("today savings summary should succeed");
    let period_savings = repository
        .summarize_usage_cost_savings(&UsageCostSavingsSummaryQuery {
            created_from_unix_secs: period.created_from_unix_secs,
            created_until_unix_secs: period.created_until_unix_secs,
            user_id: None,
            provider_name: None,
            model: None,
        })
        .await
        .expect("period savings summary should succeed");
    let legacy_elapsed = legacy_started.elapsed();

    assert_eq!(period_combined.usage, period_usage);
    assert_eq!(today_combined.usage, today_usage);
    assert_eq!(
        period_combined.cost_savings.cache_read_tokens,
        period_savings.cache_read_tokens
    );
    assert_eq!(
        today_combined.cost_savings.cache_read_tokens,
        today_savings.cache_read_tokens
    );
    eprintln!("optimized={optimized_elapsed:?} legacy={legacy_elapsed:?}");
}

#[tokio::test]
async fn repository_constructs_from_lazy_pool() {
    let factory = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url: "postgres://localhost/aether".to_string(),
        min_connections: 1,
        max_connections: 4,
        acquire_timeout_ms: 1_000,
        idle_timeout_ms: 5_000,
        max_lifetime_ms: 30_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("factory should build");

    let pool = factory.connect_lazy().expect("pool should build");
    let repository = SqlxUsageReadRepository::new(pool);
    let _ = repository.pool();
    let _ = repository.transaction_runner();
}

#[test]
fn api_key_leaderboard_user_filter_keeps_aggregates_with_historical_identity_evidence() {
    let source = include_str!("mod.rs");
    let aggregate_path = source
        .split("async fn summarize_usage_leaderboard_from_daily_aggregates")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub async fn summarize_usage_leaderboard")
                .next()
        })
        .expect("daily aggregate leaderboard path should be present");

    assert!(aggregate_path.contains("api_keys.user_id = "));
    assert!(aggregate_path.contains("api_keys.id IS NULL"));
    assert!(aggregate_path.contains("stats_daily_api_key.api_key_id IN ("));
    assert!(aggregate_path.contains("FROM usage AS identity_usage"));
    assert!(aggregate_path.contains("identity_usage.user_id = "));
    assert!(aggregate_path.contains("GROUP BY identity_usage.api_key_id"));
}

#[tokio::test]
async fn validates_upsert_before_hitting_database() {
    let factory = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url: "postgres://localhost/aether".to_string(),
        min_connections: 1,
        max_connections: 4,
        acquire_timeout_ms: 1_000,
        idle_timeout_ms: 5_000,
        max_lifetime_ms: 30_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("factory should build");

    let pool = factory.connect_lazy().expect("pool should build");
    let repository = SqlxUsageReadRepository::new(pool);
    let result = repository
        .upsert(UpsertUsageRecord {
            request_id: "".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "openai".to_string(),
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
            total_tokens: Some(30),
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
            response_time_ms: Some(100),
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
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        })
        .await;

    assert!(result.is_err());
}

#[test]
fn dashboard_daily_aggregate_split_keeps_partial_days_raw() {
    let start_utc = Utc
        .with_ymd_and_hms(2026, 4, 20, 13, 15, 0)
        .single()
        .unwrap();
    let end_utc = Utc
        .with_ymd_and_hms(2026, 4, 23, 4, 45, 0)
        .single()
        .unwrap();
    let cutoff_utc = Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).single().unwrap();

    let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);

    assert_eq!(
        split,
        AggregateRangeSplit {
            raw_leading: Some((
                Utc.with_ymd_and_hms(2026, 4, 20, 13, 15, 0)
                    .single()
                    .unwrap(),
                Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).single().unwrap(),
            )),
            aggregate: Some((
                Utc.with_ymd_and_hms(2026, 4, 21, 0, 0, 0).single().unwrap(),
                Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).single().unwrap(),
            )),
            raw_trailing: Some((
                Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).single().unwrap(),
                Utc.with_ymd_and_hms(2026, 4, 23, 4, 45, 0)
                    .single()
                    .unwrap(),
            )),
        }
    );
}

#[test]
fn dashboard_hourly_aggregate_split_keeps_partial_hours_raw() {
    let start_utc = Utc
        .with_ymd_and_hms(2026, 4, 20, 10, 15, 0)
        .single()
        .unwrap();
    let end_utc = Utc
        .with_ymd_and_hms(2026, 4, 20, 15, 30, 0)
        .single()
        .unwrap();
    let cutoff_utc = Utc
        .with_ymd_and_hms(2026, 4, 20, 15, 0, 0)
        .single()
        .unwrap();

    let split = split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc);

    assert_eq!(
        split,
        AggregateRangeSplit {
            raw_leading: Some((
                Utc.with_ymd_and_hms(2026, 4, 20, 10, 15, 0)
                    .single()
                    .unwrap(),
                Utc.with_ymd_and_hms(2026, 4, 20, 11, 0, 0)
                    .single()
                    .unwrap(),
            )),
            aggregate: Some((
                Utc.with_ymd_and_hms(2026, 4, 20, 11, 0, 0)
                    .single()
                    .unwrap(),
                Utc.with_ymd_and_hms(2026, 4, 20, 15, 0, 0)
                    .single()
                    .unwrap(),
            )),
            raw_trailing: Some((
                Utc.with_ymd_and_hms(2026, 4, 20, 15, 0, 0)
                    .single()
                    .unwrap(),
                Utc.with_ymd_and_hms(2026, 4, 20, 15, 30, 0)
                    .single()
                    .unwrap(),
            )),
        }
    );
}

#[test]
fn usage_sql_does_not_require_updated_at_column() {
    assert!(!super::FIND_BY_REQUEST_ID_SQL.contains("COALESCE(updated_at, created_at)"));
    assert!(!super::LIST_USAGE_AUDITS_PREFIX.contains("COALESCE(updated_at, created_at)"));
    assert!(!super::UPSERT_SQL.contains("\n  updated_at\n"));
    assert!(!super::UPSERT_SQL.contains("updated_at = CASE"));
}

#[test]
fn usage_sql_summarizes_tokens_by_api_key_ids_in_database() {
    let sql = super::SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL;
    assert!(sql.contains("GROUP BY api_key_id"));
    assert!(sql.contains("ANY($1::TEXT[])"));
    assert!(sql.contains("GREATEST(COALESCE(total_tokens, 0), 0)"));
    assert!(sql.contains(")::BIGINT AS total_tokens"));
    assert!(!sql.contains("COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)"));

    let source = include_str!("mod.rs");
    let implementation = source
        .split("pub async fn summarize_total_tokens_by_api_key_ids")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub async fn summarize_usage_totals_by_user_ids")
                .next()
        })
        .expect("API-key total-token summary implementation should be present");
    assert!(implementation.contains("SUM(GREATEST(COALESCE(total_tokens, 0), 0))"));
    assert!(implementation.contains(")::BIGINT AS total_tokens"));
    assert!(!implementation.contains("COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)"));
}

#[test]
fn usage_sql_summarizes_usage_by_provider_api_key_ids_in_database() {
    assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL.contains("FROM provider_api_keys"));
    assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL
        .contains("COALESCE(request_count, 0) > 0"));
    assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL.contains("ANY($1::TEXT[])"));
}

#[test]
fn usage_sql_rebuilds_provider_key_window_usage_into_status_snapshot() {
    assert!(super::REBUILD_PROVIDER_API_KEY_CODEX_WINDOW_USAGE_STATS_SQL
        .contains("UPDATE provider_api_keys AS keys"));
    assert!(super::REBUILD_PROVIDER_API_KEY_CODEX_WINDOW_USAGE_STATS_SQL
        .contains("usage_billing_facts"));
    assert!(super::REBUILD_PROVIDER_API_KEY_CODEX_WINDOW_USAGE_STATS_SQL
        .contains("provider_type', ''))) = 'codex'"));
    assert!(
        super::REBUILD_PROVIDER_API_KEY_CODEX_WINDOW_USAGE_STATS_SQL.contains("'{quota,windows}'")
    );
    assert!(super::REBUILD_PROVIDER_API_KEY_CODEX_WINDOW_USAGE_STATS_SQL.contains("'{usage}'"));
    assert!(
        super::REBUILD_PROVIDER_API_KEY_CODEX_WINDOW_USAGE_STATS_SQL.contains("WHEN '5h' THEN 300")
    );
    assert!(super::REBUILD_PROVIDER_API_KEY_CODEX_WINDOW_USAGE_STATS_SQL
        .contains("WHEN 'weekly' THEN 10080"));
}

#[test]
fn usage_sql_serializes_request_id_upserts_before_reading_previous_usage() {
    assert!(super::LOCK_USAGE_REQUEST_ID_SQL.contains("pg_advisory_xact_lock"));
    assert!(super::LOCK_USAGE_REQUEST_ID_SQL.contains("hashtext($1)::BIGINT"));
    assert!(include_str!("mod.rs")
        .contains("lock_usage_request_id_in_tx(tx, &usage.request_id).await?;"));
}

#[test]
fn usage_sql_moves_shared_counter_updates_behind_outbox() {
    let source = include_str!("mod.rs");
    assert!(super::INSERT_USAGE_COUNTER_DELTA_SQL.contains("usage_counter_deltas"));
    assert!(super::CLAIM_USAGE_COUNTER_DELTAS_SQL.contains("FOR UPDATE SKIP LOCKED"));
    assert!(super::MARK_USAGE_COUNTER_DELTAS_PROCESSED_SQL.contains("processed_at = NOW()"));
    assert!(super::TRY_LOCK_USAGE_COUNTER_FLUSH_SQL.contains("pg_try_advisory_xact_lock"));
    assert!(source.contains("enqueue_api_key_usage_delta_in_tx("));
    assert!(source.contains("enqueue_provider_api_key_usage_delta_in_tx("));
    assert!(source.contains("enqueue_model_usage_delta_in_tx("));
    assert!(source.contains("apply_provider_api_key_main_usage_delta_in_tx("));
    assert!(source.contains("USAGE_COUNTER_KIND_PROVIDER_MONTHLY"));
    assert!(source.contains("apply_provider_monthly_usage_delta_in_tx("));
}

#[test]
fn usage_sql_exposes_counter_outbox_health() {
    assert!(super::READ_USAGE_COUNTER_HEALTH_SQL.contains("pending_rows"));
    assert!(super::READ_USAGE_COUNTER_HEALTH_SQL.contains("oldest_pending_created_at_unix_secs"));
    assert!(super::READ_USAGE_COUNTER_HEALTH_SQL.contains("latest_processed_at_unix_secs"));
    assert!(super::READ_PENDING_USAGE_COUNTER_DELTAS_BY_KIND_SQL.contains("GROUP BY kind"));
}

#[test]
fn usage_counter_pending_health_does_not_scan_processed_history() {
    let sql = super::READ_USAGE_COUNTER_PENDING_HEALTH_SQL;
    assert!(sql.contains("COUNT(*)::BIGINT AS pending_rows"));
    assert!(sql.contains("MIN(created_at)"));
    assert!(sql.contains("WHERE processed_at IS NULL"));
    assert!(sql.contains("GROUP BY kind"));
    assert!(!sql.contains("processed_at IS NOT NULL"));
}

#[test]
fn usage_sql_rebuild_matches_online_api_key_usage_semantics() {
    let sql = super::REBUILD_API_KEY_USAGE_STATS_SQL;
    assert!(sql.contains("COUNT(*)::BIGINT"));
    assert!(sql.contains("GREATEST(\n        COALESCE(total_tokens, 0),"));
    assert!(!sql.contains("COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)"));
    assert!(sql.contains("AND BTRIM(api_key_id) <> ''"));
    assert!(sql.contains("AND status NOT IN ('pending', 'streaming')"));
}

#[test]
fn usage_sql_rebuild_matches_online_provider_key_usage_semantics() {
    let sql = super::REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL;
    assert!(sql.contains("COUNT(*)::BIGINT"));
    assert!(sql.contains("NULLIF(BTRIM(error_message), '') IS NULL"));
    assert!(sql.contains("GREATEST(\n          COALESCE(total_tokens, 0),"));
    assert!(!sql.contains("COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)"));
    assert!(sql.contains("AND BTRIM(provider_api_key_id) <> ''"));
    assert!(sql.contains("WHEN status NOT IN ('pending', 'streaming')"));
    assert!(sql.contains("WHEN status IN ('pending', 'streaming') THEN 0"));
}

#[test]
fn usage_sql_supports_recent_usage_audits_query() {
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("FROM \"usage\""));
}

#[test]
fn usage_sql_cache_affinity_interval_query_coalesces_nullable_model_values() {
    assert!(include_str!("mod.rs").contains("COALESCE(\"usage\".model, '') AS model"));
}

#[test]
fn usage_sql_cache_affinity_interval_query_casts_interval_minutes_to_double_precision() {
    assert!(include_str!("mod.rs").contains("AS DOUBLE PRECISION) AS interval_minutes"));
}

#[test]
fn usage_sql_summarize_usage_audits_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_audits_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("cache_creation_ephemeral_5m_tokens"));
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_cache_hit_summary_supports_global_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_cache_hit_summary_from_daily_aggregates"));
    assert!(source.contains("summarize_usage_cache_hit_summary_from_hourly_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_hourly"));
    assert!(
        source.contains("split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_cache_affinity_hit_summary_supports_global_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_cache_affinity_hit_summary_from_daily_aggregates"));
    assert!(source.contains("summarize_usage_cache_affinity_hit_summary_from_hourly_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_hourly"));
    assert!(source.contains("completed_total_requests"));
    assert!(
        source.contains("split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_settled_cost_supports_user_and_global_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_settled_cost_from_daily_aggregates"));
    assert!(source.contains("summarize_usage_settled_cost_from_hourly_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("FROM stats_hourly"));
    assert!(source.contains("FROM stats_hourly_user"));
    assert!(source.contains("settled_total_cost"));
    assert!(
        source.contains("split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_error_distribution_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_error_distribution_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily_error"));
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_performance_percentiles_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_performance_percentiles_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("p50_response_time_ms"));
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_cost_savings_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_cost_savings_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily_cost_savings"));
    assert!(source.contains("FROM stats_daily_cost_savings_model_provider"));
    assert!(source.contains("FROM stats_user_daily_cost_savings"));
    assert!(source.contains("FROM stats_user_daily_cost_savings_model_provider"));
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_time_series_supports_global_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_time_series_from_daily_aggregates"));
    assert!(source.contains("summarize_usage_time_series_from_hourly_aggregates"));
    assert!(source.contains("FROM stats_hourly"));
    assert!(
        source.contains("split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_summarize_usage_daily_heatmap_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_daily_heatmap_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("total_requests::BIGINT AS total_requests"));
    assert!(source.contains("total_cost::DOUBLE PRECISION AS total_cost"));
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_daily_cutoff_falls_back_to_imported_stats_daily() {
    let source = include_str!("mod.rs");
    assert!(source.contains("FROM stats_summary"));
    assert!(source.contains("SELECT MAX(date) AS latest_date"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("FROM stats_daily_api_key"));
    assert!(source.contains("value + chrono::Duration::days(1)"));
}

#[test]
fn usage_sql_dashboard_daily_breakdown_falls_back_to_daily_totals() {
    let source = include_str!("mod.rs");
    assert!(source.contains("list_dashboard_daily_breakdown_from_daily_totals"));
    assert!(source.contains("'aggregate'::TEXT AS model"));
    assert!(source.contains("FROM stats_daily"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("detailed_dates.contains(&item.date)"));
    assert!(source.contains("query.tz_offset_minutes != 0"));
    assert!(source.contains("return self.list_dashboard_daily_breakdown_raw(query).await;"));
    assert!(!source.contains("aggregate_dates.insert(item.date.clone())"));
}

#[test]
fn usage_sql_dashboard_daily_breakdown_keeps_all_local_day_model_provider_rows() {
    let source = include_str!("mod.rs");
    let raw_breakdown = source
        .split("async fn list_dashboard_daily_breakdown_raw")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub async fn list_dashboard_daily_breakdown")
                .next()
        })
        .expect("raw daily breakdown function should be present");
    assert!(raw_breakdown.contains("GROUP BY date, \"usage\".model, \"usage\".provider_name"));
    assert!(raw_breakdown.contains("ORDER BY date ASC, total_cost_usd DESC"));
    assert!(raw_breakdown.contains("(\"usage\".created_at AT TIME ZONE 'UTC')"));
    assert!(raw_breakdown.contains("SUM(\"usage\".total_tokens)"));
    assert!(
        !raw_breakdown.contains("split_part(lower("),
        "raw daily breakdown should aggregate the canonical total_tokens projection"
    );
    assert!(!raw_breakdown.contains("date_trunc('day', \"usage\".created_at +"));
    assert!(!source.contains("if aggregate_dates.insert(item.date.clone())"));
}

#[test]
fn usage_sql_summarize_usage_leaderboard_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("summarize_usage_leaderboard_from_daily_aggregates"));
    assert!(source.contains("FROM stats_daily_model"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("FROM stats_daily_api_key"));
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_aggregate_reads_use_materialized_total_tokens_only_when_available() {
    fn select_projections_before_table<'a>(source: &'a str, table: &str) -> Vec<&'a str> {
        let marker = format!("FROM {table}\n");
        let mut remaining = source;
        let mut projections = Vec::new();
        while let Some((before_table, after_table)) = remaining.split_once(marker.as_str()) {
            projections.push(
                before_table
                    .rsplit_once("SELECT")
                    .map(|(_, projection)| projection)
                    .unwrap_or_else(|| panic!("SELECT projection for {table} should be present")),
            );
            remaining = after_table;
        }
        projections
    }

    let source = include_str!("mod.rs");
    let breakdown = source
        .split("async fn summarize_usage_breakdown_from_daily_aggregates")
        .nth(1)
        .and_then(|tail| tail.split("async fn summarize_usage_breakdown_raw").next())
        .expect("daily aggregate usage breakdown query should be present");
    for table in [
        "stats_user_daily_model",
        "stats_user_daily_provider",
        "stats_user_daily_api_format",
    ] {
        assert!(breakdown.contains(table));
    }
    assert!(breakdown.contains("COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens"));
    assert!(!breakdown.contains(
        "SUM(effective_input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens)"
    ));

    let leaderboard = source
        .split("async fn summarize_usage_leaderboard_from_daily_aggregates")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub async fn summarize_usage_leaderboard")
                .next()
        })
        .expect("daily aggregate usage leaderboard queries should be present");
    for (table, expected_queries) in [
        ("stats_user_daily_provider", 1),
        ("stats_user_daily_model", 2),
    ] {
        let projections = select_projections_before_table(leaderboard, table);
        assert_eq!(
            projections.len(),
            expected_queries,
            "unexpected {table} query count"
        );
        assert!(projections.iter().all(|projection| projection
            .contains("COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens")));
    }

    for (table, component_expression) in [
        (
            "stats_user_daily",
            "SUM(effective_input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens)",
        ),
        (
            "stats_daily_model",
            "SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens)",
        ),
        ("stats_daily_api_key", "stats_daily_api_key.input_tokens"),
    ] {
        let projections = select_projections_before_table(leaderboard, table);
        assert_eq!(projections.len(), 1, "unexpected {table} query count");
        assert!(projections[0].contains(component_expression));
        assert!(!projections[0].contains("SUM(total_tokens)"));
    }
}

#[test]
fn usage_sql_aggregate_usage_audits_supports_daily_model_and_provider_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("aggregate_usage_audits_from_daily_aggregates"));
    assert!(source.contains("stats_user_daily_model"));
    assert!(source.contains("stats_user_daily_provider"));
    assert!(source.contains("stats_user_daily_api_format"));
    assert!(source.contains("absorb_usage_audit_aggregation_rows"));
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
}

#[test]
fn usage_sql_provider_aggregation_excludes_unknown_provider_labels() {
    let source = include_str!("mod.rs");
    assert!(source.contains("const USAGE_PROVIDER_IDENTITY_FILTER_SQL"));
    assert!(source.contains("const USAGE_PROVIDER_IDENTITY_SOURCE_SQL"));
    assert!(source.contains(r#"BTRIM(COALESCE("usage".provider_id, '')) <> ''"#));
    assert!(source.contains(r#"BTRIM(COALESCE("usage".provider_name, '')) <> ''"#));
    assert!(source.contains("LEFT JOIN providers AS provider_by_id"));
    assert!(source.contains("provider_by_id.id = BTRIM(\"usage\".provider_id)"));
    assert!(source.contains("COALESCE(\n      provider_by_id.id,\n      CASE"));
    assert!(
        source.contains(
            "ELSE BTRIM(\"usage\".provider_id)\n      END,\n      CASE\n        WHEN BTRIM(COALESCE(\"usage\".provider_name, ''))"
        )
    );
    assert!(source.contains("COALESCE(\n      provider_by_id.name,"));
    assert!(!source.contains("provider_by_name.name = BTRIM(\"usage\".provider_name)"));
    assert!(source.contains("{provider_identity_source_expr} AS provider_identity_source"));
    assert!(source.contains(r#"secondary_name_expr: "provider_identity_source""#));
    assert!(source
        .contains("COUNT(*) FILTER (WHERE secondary_name = 'provider_id') > 0 THEN 'provider_id'"));
    assert!(source.contains(
        "if matches!(query.group_by, UsageAuditAggregationGroupBy::Provider) {\n            return self.aggregate_usage_audits_raw(query).await;"
    ));
    assert!(source.contains("exclude_reserved_provider_labels"));
    assert!(source.contains(
        r#"lower(BTRIM(COALESCE(provider_name, ''))) NOT IN ('unknown', 'unknow', 'pending')"#
    ));
    assert!(source.contains("MAX(display_name)"));
    assert!(!source.contains("COALESCE(MAX(NULLIF(display_name, 'Unknown')), 'Unknown')"));
}

#[test]
fn usage_sql_summarize_total_tokens_by_api_key_ids_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("FROM stats_daily_api_key"));
    assert!(source.contains("read_stats_daily_cutoff_date().await?"));
}

#[test]
fn dashboard_aggregate_schema_mismatch_detector_matches_legacy_schema_failures() {
    assert!(super::dashboard_aggregate_schema_mismatch_message(
        "postgres error: error occurred while decoding column \"cutoff_date\": \
         mismatched types; Rust type `chrono::DateTime<Utc>` (as SQL type `TIMESTAMPTZ`) \
         is not compatible with SQL type `INT8`"
    ));
    assert!(super::dashboard_aggregate_schema_mismatch_message(
        "postgres error: db error: ERROR: relation \"stats_daily_model_provider\" does not exist"
    ));
    assert!(super::dashboard_aggregate_schema_mismatch_message(
        "postgres error: db error: ERROR: column \"effective_input_tokens\" does not exist"
    ));
    assert!(!super::dashboard_aggregate_schema_mismatch_message(
        "postgres error: db error: ERROR: permission denied for relation stats_daily"
    ));
}

#[test]
fn dashboard_aggregate_reads_fallback_to_raw_on_schema_mismatch() {
    let source = include_str!("mod.rs");
    assert!(source.contains("dashboard_should_fallback_to_raw_on_aggregate_error"));
    assert!(
        source.contains("Err(err) if dashboard_should_fallback_to_raw_on_aggregate_error(&err)")
    );
    assert!(source.contains("return self.list_dashboard_daily_breakdown_raw(query).await;"));
}

#[test]
fn usage_sql_summarize_usage_totals_by_user_ids_supports_user_summary_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("FROM stats_user_summary"));
    assert!(source.contains("all_time_input_tokens"));
    assert!(source.contains("FROM stats_user_daily"));
    assert!(source.contains("date < $2"));
    assert!(source.contains("summary_user_ids.contains(&user_id)"));
}

#[test]
fn usage_sql_raw_aggregates_use_canonical_billing_facts() {
    let source = include_str!("mod.rs");
    assert!(source.contains("FROM usage_billing_facts AS \"usage\""));
    assert!(
        super::REBUILD_API_KEY_USAGE_STATS_SQL.contains("FROM usage_billing_facts AS \"usage\"")
    );
    assert!(super::REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL
        .contains("FROM usage_billing_facts AS \"usage\""));
    assert!(super::SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL
        .contains("FROM usage_billing_facts AS \"usage\""));
    assert!(super::SUMMARIZE_USAGE_TOTALS_BY_USER_IDS_SQL
        .contains("FROM usage_billing_facts AS \"usage\""));
    assert!(!source.contains("apply_provider_api_key_codex_window_usage_delta_in_tx"));
}

#[test]
fn usage_sql_raw_token_aggregates_do_not_renormalize_canonical_billing_facts() {
    let source = include_str!("mod.rs");
    for (start, end, label) in [
        (
            "async fn summarize_usage_audits_raw",
            "pub async fn summarize_usage_audits",
            "usage audit summary",
        ),
        (
            "async fn summarize_usage_cache_affinity_hit_summary_raw",
            "async fn summarize_usage_cache_affinity_hit_summary_from_daily_aggregates",
            "cache affinity",
        ),
        (
            "async fn summarize_usage_breakdown_raw",
            "pub async fn summarize_usage_breakdown",
            "usage breakdown",
        ),
        (
            "async fn summarize_usage_leaderboard_raw",
            "async fn summarize_usage_leaderboard_from_daily_aggregates",
            "usage leaderboard",
        ),
        (
            "async fn aggregate_usage_audits_raw",
            "pub async fn aggregate_usage_audits",
            "usage audit aggregation",
        ),
        (
            "async fn summarize_usage_daily_heatmap_raw_from_range",
            "async fn summarize_usage_daily_heatmap_from_daily_aggregates",
            "usage daily heatmap",
        ),
    ] {
        let body = source
            .split(start)
            .nth(1)
            .and_then(|tail| tail.split(end).next())
            .unwrap_or_else(|| panic!("{label} raw query should be present"));
        assert!(body.contains("FROM usage_billing_facts"));
        assert!(
            !body.contains("split_part(lower("),
            "{label} must not normalize canonical billing facts by API format"
        );
        assert!(
            !body.contains("input_tokens - cache_"),
            "{label} must not subtract cache tokens from canonical input tokens"
        );
    }

    let cache_affinity = source
        .split("async fn summarize_usage_cache_affinity_hit_summary_raw")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn summarize_usage_cache_affinity_hit_summary_from_daily_aggregates")
                .next()
        })
        .expect("cache affinity raw query should be present");
    assert!(cache_affinity.contains("\"usage\".input_tokens"));
    assert!(!cache_affinity.contains("\"usage\".effective_input_tokens"));
    assert!(cache_affinity.contains("\"usage\".total_input_context"));

    let audit_summary = source
        .split("async fn summarize_usage_audits_raw")
        .nth(1)
        .and_then(|tail| tail.split("pub async fn summarize_usage_audits").next())
        .expect("usage audit summary raw query should be present");
    assert!(audit_summary.contains("SUM(GREATEST(COALESCE(\"usage\".cache_creation_input_tokens"));
    assert!(!audit_summary.contains("WHEN COALESCE(\"usage\".cache_creation_input_tokens, 0) = 0"));

    for (start, end, label) in [
        (
            "async fn summarize_usage_breakdown_raw",
            "pub async fn summarize_usage_breakdown",
            "usage breakdown",
        ),
        (
            "async fn aggregate_usage_audits_raw",
            "pub async fn aggregate_usage_audits",
            "usage audit aggregation",
        ),
    ] {
        let body = source
            .split(start)
            .nth(1)
            .and_then(|tail| tail.split(end).next())
            .unwrap_or_else(|| panic!("{label} raw query should be present"));
        assert!(body.contains("\"usage\".effective_input_tokens"));
        assert!(body.contains("\"usage\".total_input_context"));
    }

    let leaderboard = source
        .split("async fn summarize_usage_leaderboard_raw")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn summarize_usage_leaderboard_from_daily_aggregates")
                .next()
        })
        .expect("usage leaderboard raw query should be present");
    assert!(leaderboard.contains("SUM(GREATEST(COALESCE(\"usage\".total_tokens"));

    let daily_heatmap = source
        .split("async fn summarize_usage_daily_heatmap_raw_from_range")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn summarize_usage_daily_heatmap_from_daily_aggregates")
                .next()
        })
        .expect("usage daily heatmap raw query should be present");
    assert!(daily_heatmap.contains("SUM(GREATEST(COALESCE(\"usage\".total_tokens"));
    assert!(!daily_heatmap.contains("\"usage\".input_tokens + \"usage\".output_tokens"));
}

#[test]
fn usage_sql_canonical_openai_cache_case_preserves_effective_and_total_tokens() {
    let upstream_input_tokens = 166_103_i64;
    let effective_input_tokens = 1_495_i64;
    let cache_creation_tokens = 0_i64;
    let cache_read_tokens = 164_608_i64;
    let output_tokens = 94_i64;
    let total_input_context = 166_103_i64;
    let total_tokens = 166_197_i64;

    assert_eq!(
        effective_input_tokens + cache_creation_tokens + cache_read_tokens,
        total_input_context
    );
    assert_eq!(total_input_context, upstream_input_tokens);
    assert_eq!(
        effective_input_tokens + cache_creation_tokens + cache_read_tokens + output_tokens,
        total_tokens
    );

    let source = include_str!("mod.rs");
    assert!(source.contains("\"usage\".effective_input_tokens"));
    assert!(source.contains("\"usage\".total_input_context"));
    assert!(!source.contains("split_part(lower("));

    let aggregate_audit_summary = source
        .split("async fn summarize_usage_audits_from_daily_aggregates")
        .nth(1)
        .and_then(|tail| tail.split("async fn summarize_usage_audits_raw").next())
        .expect("daily aggregate usage audit summary should be present");
    assert_eq!(
        aggregate_audit_summary
            .matches("WHEN effective_input_tokens = 0 AND total_input_context = 0")
            .count(),
        2
    );
    assert_eq!(
        aggregate_audit_summary
            .matches("+ output_tokens + cache_creation_tokens + cache_read_tokens")
            .count(),
        2
    );
    assert!(!aggregate_audit_summary.contains("SUM(input_tokens + output_tokens)"));

    let raw_audit_summary = source
        .split("async fn summarize_usage_audits_raw")
        .nth(1)
        .and_then(|tail| tail.split("pub async fn summarize_usage_audits").next())
        .expect("raw usage audit summary should be present");
    assert!(raw_audit_summary.contains("SUM(GREATEST(COALESCE(\"usage\".total_tokens, 0), 0))"));
}

#[test]
fn usage_openai_total_input_context_includes_cache_creation_tokens() {
    let effective_input_tokens =
        usage_effective_input_tokens(Some(1_000), Some(100), Some(400), "openai");
    assert_eq!(effective_input_tokens, Some(500));
    assert_eq!(
        usage_total_input_context(
            Some(1_000),
            effective_input_tokens,
            Some(100),
            Some(400),
            "openai",
        ),
        Some(1_000)
    );
}

#[test]
fn dashboard_covering_index_keeps_canonical_billing_reads_heap_light() {
    let migration = include_str!(
        "../../migrations/20260715130000_add_usage_settlement_dashboard_covering_index.sql"
    );
    assert!(migration.starts_with("-- no-transaction"));
    assert!(migration.contains("CREATE INDEX CONCURRENTLY IF NOT EXISTS"));
    assert!(migration.contains("idx_usage_settlement_dashboard_cover"));
    for column in [
        "billing_input_tokens",
        "billing_effective_input_tokens",
        "billing_output_tokens",
        "billing_cache_creation_tokens",
        "billing_cache_read_tokens",
        "billing_total_input_context",
        "billing_cache_creation_cost_usd",
        "billing_cache_read_cost_usd",
        "billing_total_cost_usd",
        "billing_actual_total_cost_usd",
        "input_price_per_1m",
    ] {
        assert!(
            migration.contains(column),
            "missing covering column {column}"
        );
    }
}

#[test]
fn dashboard_combined_summary_scans_each_raw_window_once() {
    let source = include_str!("mod.rs");
    let combined = source
        .split("async fn summarize_dashboard_stats_raw")
        .nth(1)
        .and_then(|tail| tail.split("async fn summarize_dashboard_usage_raw").next())
        .expect("combined raw summary function should be present");
    assert_eq!(combined.matches("FROM usage_billing_facts").count(), 1);
    assert!(combined.contains("dashboard_eligible"));
    assert!(combined.contains("savings_estimated_full_cost_usd"));
    assert!(combined.contains("FILTER (WHERE \"usage\".dashboard_eligible)"));
    for canonical_column in [
        "effective_input_tokens",
        "total_tokens",
        "cache_creation_input_tokens",
        "total_input_context",
    ] {
        assert!(
            combined.contains(&format!("SUM(\"usage\".{canonical_column})")),
            "dashboard summary should aggregate the canonical {canonical_column} projection"
        );
    }
    assert!(
        !combined.contains("split_part(lower("),
        "dashboard summary should not recompute canonical token fields for every row"
    );
}

#[test]
fn usage_sql_provider_performance_reads_upstream_stream_from_billing_facts() {
    let source = include_str!("mod.rs");
    assert!(source.contains("\"usage\".upstream_is_stream"));
    assert!(
        !source.contains("usage_base.request_metadata->>'upstream_is_stream'"),
        "provider performance queries should not rejoin public.usage to resolve upstream stream mode"
    );
    assert!(
        !source.contains("LEFT JOIN public.usage AS usage_base"),
        "provider performance queries should stay on usage_billing_facts to avoid an extra usage scan"
    );
}

#[test]
fn usage_provider_performance_combines_summary_and_ranking_without_timeline() {
    let source = include_str!("mod.rs");
    let grouped_query = source
        .split("async fn summarize_usage_provider_performance_groups(")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn summarize_usage_provider_performance_timeline")
                .next()
        })
        .expect("provider performance grouped query should be present");
    let implementation = source
        .split("pub async fn summarize_usage_provider_performance(")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn summarize_usage_cost_savings_raw_from_range")
                .next()
        })
        .expect("provider performance implementation should be present");

    assert_eq!(grouped_query.matches("FROM usage_billing_facts").count(), 1);
    assert!(grouped_query.contains("GROUP BY GROUPING SETS ((provider_id), ())"));
    assert!(grouped_query.contains("GROUPING(provider_id)::INTEGER AS is_summary"));
    assert!(implementation.contains("if !query.include_timeline"));
    assert!(implementation.contains("summarize_usage_provider_performance_groups(query, true)"));
    assert!(implementation.contains("summary: summary.unwrap_or_default()"));
    assert!(implementation.contains("timeline: Vec::new()"));
}

#[test]
fn usage_provider_performance_parallelizes_timeline_query_chains() {
    let source = include_str!("mod.rs");
    let implementation = source
        .split("pub async fn summarize_usage_provider_performance(")
        .nth(1)
        .and_then(|tail| {
            tail.split("async fn summarize_usage_cost_savings_raw_from_range")
                .next()
        })
        .expect("provider performance implementation should be present");

    assert!(implementation.contains("let summary_future ="));
    assert!(implementation.contains("let providers_and_timeline_future = async"));
    assert!(implementation.contains("try_join(summary_future, providers_and_timeline_future)"));
    assert!(implementation.contains("let timeline = if query.include_timeline"));
    assert!(
        implementation
            .find("summarize_usage_provider_performance_groups(query, false)")
            .expect("provider ranking query should be present")
            < implementation
                .find("summarize_usage_provider_performance_timeline")
                .expect("provider timeline query should be present"),
        "timeline must keep running after provider ranking supplies its provider IDs"
    );
}

#[test]
fn usage_billing_facts_projects_upstream_stream_mode() {
    let migration = include_str!(
        "../../migrations/20260505130000_project_upstream_stream_in_usage_billing_facts.sql"
    );

    assert!(migration.contains("AS upstream_is_stream"));
    assert!(migration.contains("COALESCE(usage_rows.upstream_is_stream"));
    assert!(migration.contains("COALESCE(usage_rows.is_stream, FALSE)"));
    assert!(migration.contains("ADD COLUMN IF NOT EXISTS upstream_is_stream boolean"));
    assert!(
        !migration.contains("request_metadata->>'upstream_is_stream'"),
        "migration should avoid backfilling historical usage rows from request metadata"
    );
}

#[test]
fn usage_billing_facts_total_tokens_uses_canonical_effective_input() {
    let migration =
        include_str!("../../migrations/20260716000000_fix_usage_billing_facts_total_tokens.sql");
    let bootstrap =
        include_str!("../../../../runtime/schema/bootstrap/postgres/100_usage_capture.sql");

    for sql in [migration, bootstrap] {
        assert!(sql.contains("WHEN settlement.billing_effective_input_tokens IS NOT NULL"));
        assert!(sql.contains("GREATEST(settlement.billing_effective_input_tokens, 0)"));
        assert!(sql.contains("WHEN settlement.billing_total_input_context IS NOT NULL"));
        assert!(sql.contains("NULLIF(GREATEST(COALESCE(usage_rows.total_tokens, 0), 0), 0)"));
        assert!(!sql.contains("THEN COALESCE(settlement.billing_input_tokens, 0)"));
    }
}

#[test]
fn usage_sql_reads_http_audits_for_single_record_fetches() {
    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("LEFT JOIN usage_http_audits"));
    assert!(super::FIND_BY_ID_SQL.contains("LEFT JOIN usage_http_audits"));
    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("http_request_body_ref"));
    assert!(super::FIND_BY_ID_SQL.contains("http_client_response_body_ref"));
}

#[test]
fn usage_sql_reads_routing_snapshots_for_single_record_fetches() {
    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("LEFT JOIN usage_routing_snapshots"));
    assert!(super::FIND_BY_ID_SQL.contains("LEFT JOIN usage_routing_snapshots"));
    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("routing_candidate_id"));
    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("routing_candidate_index"));
    assert!(super::FIND_BY_ID_SQL.contains("routing_local_execution_runtime_miss_reason"));
}

#[test]
fn usage_sql_reads_settlement_snapshots_for_single_record_fetches() {
    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("LEFT JOIN usage_settlement_snapshots"));
    assert!(super::FIND_BY_ID_SQL.contains("LEFT JOIN usage_settlement_snapshots"));
    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("settlement_billing_snapshot_schema_version"));
    assert!(super::FIND_BY_ID_SQL.contains("settlement_price_per_request"));
    for sql in [
        super::FIND_BY_REQUEST_ID_SQL,
        super::FIND_BY_ID_SQL,
        super::LIST_USAGE_AUDITS_PREFIX,
        super::LIST_RECENT_USAGE_AUDITS_PREFIX,
    ] {
        assert!(sql.contains("CAST(\"usage\".input_tokens AS INTEGER) AS input_tokens"));
        assert!(sql.contains(
            "usage_settlement_snapshots.billing_input_tokens AS settlement_billing_input_tokens"
        ));
        assert!(sql.contains(
            "WHEN usage_settlement_snapshots.billing_effective_input_tokens IS NOT NULL"
        ));
        assert!(
            sql.contains("WHEN usage_settlement_snapshots.billing_total_input_context IS NOT NULL")
        );
        assert!(sql.contains("NULLIF(GREATEST(COALESCE(\"usage\".total_tokens, 0), 0), 0)"));
        assert!(!sql.contains("THEN COALESCE(usage_settlement_snapshots.billing_input_tokens, 0)"));
        assert!(sql.contains("usage_settlement_snapshots.billing_cache_creation_5m_tokens"));
        assert!(sql.contains(
            "CAST(usage_settlement_snapshots.billing_total_cost_usd AS DOUBLE PRECISION)"
        ));
    }
}

#[test]
fn usage_sql_qualifies_shared_usage_columns_for_single_record_fetches() {
    for sql in [super::FIND_BY_REQUEST_ID_SQL, super::FIND_BY_ID_SQL] {
        assert!(sql.contains("\"usage\".request_id"));
        assert!(
                sql.contains(
                    "COALESCE(usage_settlement_snapshots.billing_status, \"usage\".billing_status) AS billing_status"
                )
            );
        assert!(sql
            .contains("CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION)"));
        assert!(sql.contains("EXTRACT(EPOCH FROM \"usage\".created_at)"));
        assert!(sql.contains("usage_settlement_snapshots.finalized_at"));
        assert!(!sql.contains("CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT)"));
        assert!(!sql.contains("CAST(output_price_per_1m AS DOUBLE PRECISION)"));
    }

    assert!(super::FIND_BY_REQUEST_ID_SQL.contains("WHERE \"usage\".request_id = $1"));
    assert!(super::FIND_BY_ID_SQL.contains("WHERE \"usage\".id = $1"));
}

#[test]
fn usage_sql_uses_json_null_placeholders_for_usage_payload_columns() {
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::json AS request_headers"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::json AS provider_request_body"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::bytea AS request_body_compressed"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::varchar AS http_request_body_ref"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::varchar AS http_request_body_state"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX
        .contains("NULL::varchar AS http_client_response_body_state"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("usage_routing_snapshots.candidate_id"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("usage_routing_snapshots.candidate_index"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("request_metadata->>'candidate_index'"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("LEFT JOIN usage_routing_snapshots"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("LEFT JOIN usage_settlement_snapshots"));
    assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("settlement_billing_snapshot_schema_version"));
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::json AS request_headers"));
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::json AS provider_request_body"));
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
        .contains("NULL::bytea AS client_response_body_compressed"));
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
        .contains("NULL::varchar AS http_client_response_body_ref"));
    assert!(
        super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::varchar AS http_request_body_state")
    );
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
        .contains("NULL::varchar AS http_client_response_body_state"));
    assert!(
        super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("usage_routing_snapshots.candidate_index")
    );
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("request_metadata->>'candidate_index'"));
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("LEFT JOIN usage_routing_snapshots"));
    assert!(
        super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("usage_routing_snapshots.execution_path")
    );
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("LEFT JOIN usage_settlement_snapshots"));
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("settlement_price_per_request"));
    for sql in [
        super::LIST_USAGE_AUDITS_PREFIX,
        super::LIST_RECENT_USAGE_AUDITS_PREFIX,
    ] {
        assert!(sql.contains("jsonb_strip_nulls(jsonb_build_object("));
        assert!(!sql.contains("jsonb_typeof(\"usage\".provider_request_body::jsonb)"));
        assert!(!sql.contains("\"usage\".provider_request_body->"));
        assert!(!sql.contains("jsonb_typeof(\"usage\".provider_request_body)"));
        assert!(sql.contains("'client_ip'"));
        assert!(sql.contains("request_metadata->>'client_ip'"));
        assert!(sql.contains("'user_agent'"));
        assert!(sql.contains("request_metadata->>'user_agent'"));
        assert!(sql.contains("request_metadata->>'requested_reasoning_effort'"));
        assert!(sql.contains("request_metadata->>'provider_reasoning_effort'"));
        assert!(sql.contains("request_metadata->>'provider_service_tier'"));
        assert!(sql.contains("request_metadata->>'provider_actual_service_tier'"));
        assert!(sql.contains("AS client_family"));
        assert!(sql.contains("request_metadata->'client_session_affinity'->>'client_family'"));
        assert!(sql.contains("request_metadata->>'client_family'"));
        assert!(sql.contains("CAST(\"usage\".input_tokens AS INTEGER) AS input_tokens"));
        assert!(sql.contains(
            "usage_settlement_snapshots.billing_input_tokens AS settlement_billing_input_tokens"
        ));
        assert!(sql.contains("usage_settlement_snapshots.billing_cache_creation_1h_tokens"));
        assert!(sql.contains(
            "CAST(usage_settlement_snapshots.billing_total_cost_usd AS DOUBLE PRECISION)"
        ));
    }
    assert!(!super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::jsonb"));
    assert!(!super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::jsonb"));
}

#[test]
fn usage_sql_admin_record_filters_are_pushed_into_postgres_queries() {
    let source = include_str!("mod.rs");
    assert!(source.contains("push_postgres_usage_client_family_filter"));
    assert!(source.contains("request_metadata->'client_session_affinity'->>'client_family'"));
    assert!(source.contains("request_metadata->>'client_family'"));
    assert!(source.contains("exclude_unknown_model_or_provider"));
    assert!(source.contains("NOT IN ('unknown', 'unknow')"));
}

#[test]
fn usage_sql_keyword_search_error_filter_keeps_where_state_before_keywords() {
    let source = include_str!("mod.rs");
    let function = source
        .split("pub async fn list_usage_audits_by_keyword_search")
        .nth(1)
        .and_then(|tail| tail.split("pub async fn count_usage_audits").next())
        .expect("keyword search function should be present");
    let error_filter = function
        .split("if query.error_only")
        .nth(1)
        .and_then(|tail| tail.split("for (index, keyword)").next())
        .expect("error filter should precede keyword loop");

    assert!(error_filter.contains("has_where = true;"));
}

#[test]
fn usage_sql_reads_list_output_price_from_settlement_snapshots_before_legacy_usage_column() {
    assert!(super::LIST_USAGE_AUDITS_PREFIX
        .contains("CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION)"));
    assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
        .contains("CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION)"));
}

#[test]
fn usage_sql_casts_json_payload_bind_parameters_explicitly() {
    for placeholder in [41, 42, 44, 45, 47, 48, 50, 51, 53] {
        assert!(
            super::UPSERT_SQL.contains(format!("${placeholder}::json").as_str()),
            "missing ::json cast for placeholder ${placeholder}"
        );
    }
}

#[test]
fn usage_sql_insert_values_aligns_request_metadata_and_timestamps() {
    assert!(super::UPSERT_SQL.contains("\n  $51::json,\n  $52,\n  $53::json,\n  CASE"));
    assert!(super::UPSERT_SQL.contains("WHEN $54 IS NULL THEN NULL"));
    assert!(super::UPSERT_SQL.contains("TO_TIMESTAMP($55::double precision)"));
}

#[test]
fn usage_sql_upsert_materializes_upstream_stream_mode() {
    assert!(super::UPSERT_SQL.contains("upstream_is_stream,"));
    assert!(super::UPSERT_SQL.contains("$53::json->>'upstream_is_stream'"));
    assert!(super::UPSERT_SQL.contains("COALESCE($21, FALSE)"));
    assert!(super::UPSERT_SQL.contains("upstream_is_stream = CASE"));
}

#[test]
fn usage_sql_upsert_returning_includes_routing_placeholders() {
    assert!(super::UPSERT_SQL.contains("NULL::varchar AS http_request_body_state"));
    assert!(super::UPSERT_SQL.contains("NULL::varchar AS http_client_response_body_state"));
    assert!(super::UPSERT_SQL.contains("NULL::varchar AS routing_candidate_id"));
    assert!(super::UPSERT_SQL.contains("NULL::varchar AS routing_planner_kind"));
    assert!(super::UPSERT_SQL.contains("NULL::varchar AS routing_execution_path"));
    assert!(
        super::UPSERT_SQL.contains("NULL::varchar AS settlement_billing_snapshot_schema_version")
    );
    assert!(super::UPSERT_SQL.contains("NULL::double precision AS settlement_input_price_per_1m"));
    assert!(super::UPSERT_SQL.contains("input_output_total_tokens"));
    assert!(super::UPSERT_SQL.contains("input_context_tokens"));
}

#[test]
fn usage_sql_writes_usage_settlement_pricing_snapshots() {
    assert!(super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL
        .contains("INSERT INTO usage_settlement_snapshots"));
    assert!(super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL
        .contains("billing_snapshot_schema_version"));
    assert!(super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL.contains("price_per_request"));
}

#[test]
fn usage_sql_settlement_pricing_snapshot_billing_values_use_authoritative_incoming_values() {
    let sql = super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL;
    for field in [
        "billing_input_tokens",
        "billing_effective_input_tokens",
        "billing_output_tokens",
        "billing_cache_creation_tokens",
        "billing_cache_creation_5m_tokens",
        "billing_cache_creation_1h_tokens",
        "billing_cache_read_tokens",
        "billing_total_input_context",
        "billing_cache_creation_cost_usd",
        "billing_cache_read_cost_usd",
        "billing_total_cost_usd",
        "billing_actual_total_cost_usd",
    ] {
        let assignment = format!(
            "{field} = CASE WHEN $30 THEN EXCLUDED.{field} ELSE COALESCE(EXCLUDED.{field}, usage_settlement_snapshots.{field}) END"
        );
        assert!(
            sql.contains(assignment.as_str()),
            "missing authoritative billing snapshot assignment: {assignment}"
        );
        assert!(
            !sql.contains(format!("{field} = GREATEST(").as_str()),
            "billing snapshot field should not use max-only conflict resolution: {field}"
        );
    }
}

#[test]
fn usage_sql_upsert_recovers_missing_provider_links_after_billing_finalizes() {
    for assignment in [
        "provider_id = CASE WHEN (\"usage\".billing_status = 'pending' AND $61) OR (\"usage\".billing_status <> 'pending' AND \"usage\".provider_id IS NULL AND (\"usage\".provider_endpoint_id IS NULL OR \"usage\".provider_endpoint_id = EXCLUDED.provider_endpoint_id) AND (\"usage\".provider_api_key_id IS NULL OR \"usage\".provider_api_key_id = EXCLUDED.provider_api_key_id)) THEN COALESCE(EXCLUDED.provider_id, \"usage\".provider_id) ELSE \"usage\".provider_id END",
        "provider_endpoint_id = CASE WHEN (\"usage\".billing_status = 'pending' AND $61) OR (\"usage\".billing_status <> 'pending' AND \"usage\".provider_endpoint_id IS NULL AND (\"usage\".provider_id IS NULL OR \"usage\".provider_id = EXCLUDED.provider_id) AND (\"usage\".provider_api_key_id IS NULL OR \"usage\".provider_api_key_id = EXCLUDED.provider_api_key_id)) THEN COALESCE(EXCLUDED.provider_endpoint_id, \"usage\".provider_endpoint_id) ELSE \"usage\".provider_endpoint_id END",
        "provider_api_key_id = CASE WHEN (\"usage\".billing_status = 'pending' AND $61) OR (\"usage\".billing_status <> 'pending' AND \"usage\".provider_api_key_id IS NULL AND (\"usage\".provider_id IS NULL OR \"usage\".provider_id = EXCLUDED.provider_id) AND (\"usage\".provider_endpoint_id IS NULL OR \"usage\".provider_endpoint_id = EXCLUDED.provider_endpoint_id)) THEN COALESCE(EXCLUDED.provider_api_key_id, \"usage\".provider_api_key_id) ELSE \"usage\".provider_api_key_id END",
    ] {
        assert!(
            super::UPSERT_SQL.contains(assignment),
            "missing provider link recovery assignment: {assignment}"
        );
    }
}

#[test]
fn usage_sql_updates_usage_mirror_columns_from_terminal_events_only() {
    for field in [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "input_output_total_tokens",
        "input_context_tokens",
        "cache_creation_input_tokens",
        "cache_creation_input_tokens_5m",
        "cache_creation_input_tokens_1h",
        "cache_read_input_tokens",
        "cache_creation_cost_usd",
        "cache_read_cost_usd",
        "total_cost_usd",
        "actual_total_cost_usd",
    ] {
        let assignment = format!(
            "{field} = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".{field}, EXCLUDED.{field}) ELSE \"usage\".{field} END"
        );
        assert!(
            super::UPSERT_SQL.contains(assignment.as_str()),
            "missing terminal mirror assignment: {assignment}"
        );
        assert!(
            !super::UPSERT_SQL.contains(format!("{field} = CASE WHEN EXCLUDED.status IN").as_str()),
            "terminal mirror assignment must keep pending billing guard: {field}"
        );
    }
}

#[test]
fn usage_sql_binds_usage_mirror_values_for_terminal_upserts() {
    let source = include_str!("mod.rs");
    for binding in [
        ".bind(usage.input_tokens.map(to_i32).transpose()?)",
        ".bind(usage.output_tokens.map(to_i32).transpose()?)",
        ".bind(usage.total_tokens.map(to_i32).transpose()?)",
        ".bind(usage.cache_creation_input_tokens.map(to_i32).transpose()?)",
        ".bind(usage.cache_read_input_tokens.map(to_i32).transpose()?)",
        ".bind(usage.cache_creation_cost_usd)",
        ".bind(usage.cache_read_cost_usd)",
        ".bind(usage.total_cost_usd)",
        ".bind(usage.actual_total_cost_usd)",
    ] {
        assert!(
            source.contains(binding),
            "missing usage upsert bind: {binding}"
        );
    }
}

#[test]
fn usage_sql_clears_legacy_output_price_column_on_upsert() {
    assert!(super::UPSERT_SQL.contains("output_price_per_1m = NULL"));
    assert!(include_str!("mod.rs").contains(".bind(None::<f64>)"));
}

#[test]
fn usage_sql_clears_legacy_header_columns_on_upsert() {
    assert!(super::UPSERT_SQL.contains("request_headers = NULL"));
    assert!(super::UPSERT_SQL.contains("provider_request_headers = NULL"));
    assert!(super::UPSERT_SQL.contains("response_headers = NULL"));
    assert!(super::UPSERT_SQL.contains("client_response_headers = NULL"));
}

#[test]
fn usage_sql_detached_body_flags_clear_inline_and_compressed_columns() {
    assert!(super::UPSERT_SQL
        .contains("WHEN EXCLUDED.request_body_compressed IS NOT NULL OR $57 THEN NULL"));
    assert!(super::UPSERT_SQL
        .contains("WHEN EXCLUDED.provider_request_body_compressed IS NOT NULL OR $58 THEN NULL"));
    assert!(super::UPSERT_SQL
        .contains("WHEN EXCLUDED.response_body_compressed IS NOT NULL OR $59 THEN NULL"));
    assert!(super::UPSERT_SQL
        .contains("WHEN EXCLUDED.client_response_body_compressed IS NOT NULL OR $60 THEN NULL"));
}

#[test]
fn usage_sql_capture_guard_covers_bodies_metadata_and_http_ref_tombstones() {
    for assignment in [
        "request_body = CASE WHEN \"usage\".billing_status = 'pending' AND $61",
        "provider_request_body = CASE WHEN \"usage\".billing_status = 'pending' AND $61",
        "response_body = CASE WHEN \"usage\".billing_status = 'pending' AND $61",
        "client_response_body = CASE WHEN \"usage\".billing_status = 'pending' AND $61",
        "request_metadata = CASE WHEN \"usage\".billing_status = 'pending' AND $61",
    ] {
        assert!(
            super::UPSERT_SQL.contains(assignment),
            "missing guard: {assignment}"
        );
    }
    for assignment in [
        "WHEN EXCLUDED.request_body_state = 'none' THEN NULL",
        "WHEN EXCLUDED.provider_request_body_state = 'none' THEN NULL",
        "WHEN EXCLUDED.response_body_state = 'none' THEN NULL",
        "WHEN EXCLUDED.client_response_body_state = 'none' THEN NULL",
    ] {
        assert!(
            super::UPSERT_USAGE_HTTP_AUDIT_SQL.contains(assignment),
            "missing HTTP ref tombstone: {assignment}"
        );
    }
    let source = include_str!("mod.rs");
    assert!(source.contains(".bind(capture_update_allowed)"));
    assert!(source.contains("if capture_update_allowed {"));
}

#[test]
fn usage_sql_terminal_snapshots_replace_sparse_routing_and_settlement_facts() {
    assert!(super::UPSERT_SQL.contains(
        "target_model = CASE WHEN \"usage\".billing_status = 'pending' AND $61 THEN CASE WHEN EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN EXCLUDED.target_model"
    ));
    let routing = super::UPSERT_USAGE_ROUTING_SNAPSHOT_SQL;
    assert!(routing.contains("candidate_id = CASE WHEN $14 THEN EXCLUDED.candidate_id"));
    assert!(
        routing.contains("selected_provider_id = CASE WHEN $14 THEN EXCLUDED.selected_provider_id")
    );
    let settlement = super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL;
    assert!(settlement
        .contains("settlement_snapshot = CASE WHEN $30 THEN EXCLUDED.settlement_snapshot"));
    assert!(
        settlement.contains("billing_dimensions = CASE WHEN $30 THEN EXCLUDED.billing_dimensions")
    );
    assert!(settlement.contains("rate_multiplier = CASE WHEN $30 THEN EXCLUDED.rate_multiplier"));
    assert!(include_str!("mod.rs").contains(".bind(replace_existing)"));
}

#[test]
fn usage_sql_clears_stale_failure_fields_for_non_failed_status_updates() {
    assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') AND EXCLUDED.status_code IS NULL THEN NULL"
        ));
    assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_message"
        ));
    assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_category"
        ));
}

#[test]
fn stale_cleanup_failed_candidate_sql_orders_by_effective_timestamp() {
    let sql = super::SELECT_LATEST_FAILED_CANDIDATE_FOR_STALE_REQUESTS_SQL;
    assert!(sql.contains("COALESCE(finished_at, started_at, created_at) DESC"));
    assert!(!sql.contains("finished_at DESC NULLS LAST"));
    assert!(!sql.contains("started_at DESC NULLS LAST"));
}

#[test]
fn usage_sql_does_not_allow_streaming_to_regress_back_to_pending() {
    assert!(super::UPSERT_SQL.contains(
            "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'pending' THEN \"usage\".status_code"
        ));
    assert!(super::UPSERT_SQL.contains(
            "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'streaming' AND EXCLUDED.status_code IS NULL THEN \"usage\".status_code"
        ));
    assert!(super::UPSERT_SQL.contains(
            "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'pending' THEN \"usage\".error_message"
        ));
    assert!(super::UPSERT_SQL.contains(
        "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'pending' THEN \"usage\".status"
    ));
}

#[test]
fn usage_sql_recovers_void_failures_before_upsert_and_settlement() {
    assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("UPDATE \"usage\""));
    assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("billing_status = 'pending'"));
    assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("finalized_at = NULL"));
    assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("status IN ('failed', 'cancelled')"));
    assert!(super::RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL
        .contains("UPDATE usage_settlement_snapshots"));
    assert!(super::RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL
        .contains("billing_status = 'pending'"));
    assert!(super::RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL.contains("finalized_at = NULL"));
}

#[test]
fn prepare_usage_body_storage_detaches_small_payloads_into_blob_storage() {
    let payload = json!({"message": "hello"});
    let storage = prepare_usage_body_storage(Some(&payload)).expect("storage should serialize");

    assert!(storage.inline_json.is_none());
    let compressed = storage
        .detached_blob_bytes
        .as_deref()
        .expect("small payload should now be ref-backed");
    assert_eq!(
        inflate_usage_json_value(compressed).expect("payload should inflate"),
        payload
    );
}

#[test]
fn prepare_usage_body_storage_compresses_large_payloads() {
    let payload = json!({
        "content": "x".repeat(MAX_INLINE_USAGE_BODY_BYTES + 128)
    });
    let storage = prepare_usage_body_storage(Some(&payload)).expect("storage should serialize");

    assert!(storage.inline_json.is_none());
    let compressed = storage
        .detached_blob_bytes
        .as_deref()
        .expect("large payload should be compressed");
    assert_eq!(
        inflate_usage_json_value(compressed).expect("payload should inflate"),
        payload
    );
}

#[test]
fn usage_body_capture_state_for_storage_marks_detached_bodies_as_reference() {
    let payload = json!({"message": "hello"});
    let storage = prepare_usage_body_storage(Some(&payload)).expect("storage should serialize");

    assert!(storage.has_detached_blob());
    assert_eq!(
        usage_body_capture_state_for_storage(Some(UsageBodyCaptureState::Inline), &storage, None,),
        Some(UsageBodyCaptureState::Reference)
    );
    assert_eq!(
        usage_body_capture_state_for_storage(None, &storage, None),
        Some(UsageBodyCaptureState::Reference)
    );
}

#[test]
fn usage_body_capture_state_for_storage_preserves_unavailable_states() {
    let payload = json!({"message": "hello"});
    let storage = prepare_usage_body_storage(Some(&payload)).expect("storage should serialize");

    assert_eq!(
        usage_body_capture_state_for_storage(
            Some(UsageBodyCaptureState::Disabled),
            &storage,
            Some("usage://request/req-1/request_body"),
        ),
        Some(UsageBodyCaptureState::Disabled)
    );
    assert_eq!(
        usage_body_capture_state_for_storage(
            Some(UsageBodyCaptureState::Unavailable),
            &storage,
            Some("usage://request/req-1/request_body"),
        ),
        Some(UsageBodyCaptureState::Unavailable)
    );
}

#[test]
fn explicit_none_capture_replaces_stale_request_body_facts_even_with_a_residual_body() {
    let stale_body = json!({
        "reasoning_effort": "xhigh",
        "service_tier": "priority"
    });

    assert!(request_body_capture_replaces_derived_facts(
        Some(&stale_body),
        Some(UsageBodyCaptureState::None),
    ));
    assert!(request_body_capture_replaces_derived_facts(
        Some(&stale_body),
        None,
    ));
    assert!(request_body_capture_replaces_derived_facts(
        Some(&stale_body),
        Some(UsageBodyCaptureState::Disabled),
    ));
    assert!(request_body_capture_replaces_derived_facts(
        None,
        Some(UsageBodyCaptureState::Truncated),
    ));
    assert!(!request_body_capture_replaces_derived_facts(None, None,));
}

#[test]
fn explicit_none_capture_drops_residual_body_ref_and_incoming_fast_metadata_before_storage() {
    let mut usage = fast_clear_usage_record(
        "req-none-residual",
        "none-residual",
        100,
        true,
        UsageBodyCaptureState::None,
        Some("priority"),
    );
    usage.provider_request_body = Some(json!({
        "model": "gpt-5",
        "service_tier": "priority"
    }));
    usage.provider_request_body_ref =
        Some("usage://request/req-none-residual/provider_request_body".to_string());
    usage.request_metadata = Some(json!({
        "trace_id": "trace-1",
        "provider_service_tier": "priority",
        "provider_reasoning_effort": "high",
        "provider_cache_ttl_minutes": 30,
        "provider_request_body_ref": "usage://request/req-none-residual/provider_request_body"
    }));

    let prepared = prepare_usage_upsert_context(&usage).expect("usage should prepare");
    assert!(prepared.clear_provider_request_body);
    assert!(!prepared.provider_request_body_storage.has_detached_blob());
    assert_eq!(prepared.http_audit_refs.provider_request_body_ref, None);
    assert_eq!(
        prepared.request_metadata_value,
        Some(json!({"trace_id": "trace-1"}))
    );
}

#[test]
fn request_body_fact_clear_keeps_unrelated_metadata_and_emits_an_empty_tombstone() {
    let previous = json!({
        "trace_id": "trace-1",
        "requested_reasoning_effort": "xhigh",
        "provider_reasoning_effort": "max",
        "provider_service_tier": "priority",
        "provider_cache_ttl_minutes": 30,
        "provider_actual_service_tier": "default"
    });

    assert_eq!(
        clear_previous_request_body_facts(Some(&previous), true, true),
        json!({
            "trace_id": "trace-1",
            "provider_actual_service_tier": "default"
        })
    );
    assert_eq!(
        clear_previous_request_body_facts(
            Some(&json!({"provider_service_tier": "priority"})),
            false,
            true,
        ),
        json!({})
    );
}

#[test]
fn terminal_capture_rejects_late_non_terminal_updates() {
    assert!(usage_capture_update_allowed(None, "pending"));
    assert!(usage_capture_update_allowed(
        Some(("pending", "pending")),
        "streaming",
    ));
    assert!(usage_capture_update_allowed(
        Some(("failed", "pending")),
        "completed",
    ));
    assert!(!usage_capture_update_allowed(
        Some(("completed", "pending")),
        "pending",
    ));
    assert!(!usage_capture_update_allowed(
        Some(("failed", "pending")),
        "streaming",
    ));
    assert!(!usage_capture_update_allowed(
        Some(("streaming", "pending")),
        "pending",
    ));
    assert!(!usage_capture_update_allowed(
        Some(("completed", "settled")),
        "completed",
    ));
}

#[test]
fn prepare_request_metadata_for_body_storage_strips_body_ref_compatibility_keys() {
    let detached = prepare_usage_body_storage(Some(&json!({
        "content": "x".repeat(MAX_INLINE_USAGE_BODY_BYTES + 32)
    })))
    .expect("detached storage should build");
    let inline =
        prepare_usage_body_storage(Some(&json!({"message": "inline"}))).expect("inline body");

    let metadata = prepare_request_metadata_for_body_storage(
        Some(json!({
            "trace_id": "trace-1",
            "request_body_ref": "blob://old-request",
            "provider_request_body_ref": "blob://old-provider"
        })),
        [
            (
                UsageBodyField::RequestBody,
                &detached,
                Some(&json!({"request": true})),
                Some("usage://request/req-123/request_body"),
            ),
            (
                UsageBodyField::ProviderRequestBody,
                &inline,
                Some(&json!({"provider": true})),
                None,
            ),
        ],
    )
    .expect("metadata should be present");

    assert_eq!(
        metadata,
        json!({
            "trace_id": "trace-1"
        })
    );
}

#[test]
fn attach_compressed_body_refs_adds_missing_ref_metadata() {
    let metadata = attach_compressed_body_refs(
        "req-123",
        Some(json!({
            "candidate_id": "cand-1",
            "provider_request_body_ref": "blob://existing"
        })),
        true,
        true,
        true,
        false,
    )
    .expect("metadata should remain");

    assert_eq!(
        metadata,
        json!({
            "candidate_id": "cand-1",
            "request_body_ref": usage_body_ref("req-123", UsageBodyField::RequestBody),
            "provider_request_body_ref": "blob://existing",
            "response_body_ref": usage_body_ref("req-123", UsageBodyField::ResponseBody)
        })
    );
}

#[test]
fn usage_http_audit_body_refs_extracts_only_non_empty_values() {
    let refs = usage_http_audit_body_refs(Some(&json!({
        "request_body_ref": "usage://request/req-123/request_body",
        "provider_request_body_ref": "  ",
        "response_body_ref": "usage://request/req-123/response_body"
    })));

    assert_eq!(
        refs,
        UsageHttpAuditRefs {
            request_body_ref: Some("usage://request/req-123/request_body".to_string()),
            provider_request_body_ref: None,
            response_body_ref: Some("usage://request/req-123/response_body".to_string()),
            client_response_body_ref: None,
        }
    );
}

#[test]
fn resolved_read_usage_body_ref_prefers_typed_then_http_audit_then_compressed_then_metadata() {
    let metadata = json!({
        "request_body_ref": "usage://request/req-123/request_body"
    });
    let invalid_metadata = json!({
        "request_body_ref": "blob://metadata-request"
    });
    let mismatched_metadata = json!({
        "request_body_ref": "usage://request/req-other/request_body"
    });

    assert_eq!(
        resolved_read_usage_body_ref(
            Some("usage://request/req-123/request_body"),
            metadata.as_object(),
            "req-123",
            UsageBodyField::RequestBody,
            true,
            Some("usage://request/req-123/request_body"),
        ),
        Some("usage://request/req-123/request_body".to_string())
    );
    assert_eq!(
        resolved_read_usage_body_ref(
            None,
            metadata.as_object(),
            "req-123",
            UsageBodyField::RequestBody,
            false,
            Some("usage://request/req-123/request_body"),
        ),
        Some("usage://request/req-123/request_body".to_string())
    );
    assert_eq!(
        resolved_read_usage_body_ref(
            None,
            metadata.as_object(),
            "req-123",
            UsageBodyField::RequestBody,
            true,
            None,
        ),
        Some(usage_body_ref("req-123", UsageBodyField::RequestBody))
    );
    assert_eq!(
        resolved_read_usage_body_ref(
            None,
            invalid_metadata.as_object(),
            "req-123",
            UsageBodyField::RequestBody,
            false,
            None,
        ),
        None
    );
    assert_eq!(
        resolved_read_usage_body_ref(
            None,
            mismatched_metadata.as_object(),
            "req-123",
            UsageBodyField::RequestBody,
            false,
            None,
        ),
        None
    );
    assert_eq!(
        resolved_read_usage_body_ref(
            None,
            None,
            "req-123",
            UsageBodyField::ResponseBody,
            true,
            Some("usage://request/req-123/response_body"),
        ),
        Some(usage_body_ref("req-123", UsageBodyField::ResponseBody))
    );
    assert_eq!(
        resolved_read_usage_body_ref(
            None,
            None,
            "req-123",
            UsageBodyField::ClientResponseBody,
            false,
            Some("usage://request/req-123/client_response_body"),
        ),
        Some("usage://request/req-123/client_response_body".to_string())
    );
}

#[test]
fn resolved_write_usage_body_ref_ignores_metadata_compatibility_keys() {
    assert_eq!(
        resolved_write_usage_body_ref(None, "req-123", UsageBodyField::RequestBody, false, None,),
        None
    );
    assert_eq!(
        resolved_write_usage_body_ref(
            Some("usage://request/req-123/request_body"),
            "req-123",
            UsageBodyField::RequestBody,
            true,
            Some("usage://request/req-123/request_body"),
        ),
        Some("usage://request/req-123/request_body".to_string())
    );
    assert_eq!(
        resolved_write_usage_body_ref(
            None,
            "req-123",
            UsageBodyField::ResponseBody,
            true,
            Some("usage://request/req-123/response_body"),
        ),
        Some(usage_body_ref("req-123", UsageBodyField::ResponseBody))
    );
    assert_eq!(
        resolved_write_usage_body_ref(
            None,
            "req-123",
            UsageBodyField::ClientResponseBody,
            false,
            Some("usage://request/req-123/client_response_body"),
        ),
        Some("usage://request/req-123/client_response_body".to_string())
    );
}

#[test]
fn usage_http_audit_capture_mode_prefers_refs_over_inline_legacy() {
    let refs = UsageHttpAuditRefs {
        request_body_ref: Some("usage://request/req-123/request_body".to_string()),
        ..UsageHttpAuditRefs::default()
    };
    assert_eq!(
        usage_http_audit_capture_mode(&refs, [Some(&json!({"request": true})), None, None, None]),
        "ref_backed"
    );
    assert_eq!(
        usage_http_audit_capture_mode(
            &UsageHttpAuditRefs::default(),
            [Some(&json!({"request": true})), None, None, None]
        ),
        "inline_legacy"
    );
    assert_eq!(
        usage_http_audit_capture_mode(&UsageHttpAuditRefs::default(), [None, None, None, None]),
        "none"
    );
}

#[test]
fn attach_usage_http_audit_body_refs_adds_missing_metadata_without_overwriting_existing_keys() {
    let metadata = attach_usage_http_audit_body_refs(
        Some(json!({
            "candidate_id": "cand-1",
            "request_body_ref": "blob://existing"
        })),
        &UsageHttpAuditRefs {
            request_body_ref: Some("usage://request/req-123/request_body".to_string()),
            provider_request_body_ref: Some(
                "usage://request/req-123/provider_request_body".to_string(),
            ),
            response_body_ref: None,
            client_response_body_ref: Some(
                "usage://request/req-123/client_response_body".to_string(),
            ),
        },
    )
    .expect("metadata should remain");

    assert_eq!(
        metadata,
        json!({
            "candidate_id": "cand-1",
            "request_body_ref": "blob://existing",
            "provider_request_body_ref": "usage://request/req-123/provider_request_body",
            "client_response_body_ref": "usage://request/req-123/client_response_body"
        })
    );
}

#[test]
fn usage_routing_snapshot_from_usage_only_activates_for_routing_metadata() {
    let snapshot = usage_routing_snapshot_from_usage(
        &UpsertUsageRecord {
            request_id: "req-123".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "openai".to_string(),
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
            input_tokens: Some(1),
            output_tokens: Some(2),
            total_tokens: Some(3),
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
            response_time_ms: Some(100),
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
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 100,
        },
        Some(&json!({
            "candidate_id": "cand-1",
            "key_name": "primary",
            "planner_kind": "claude_cli_sync",
            "route_family": "claude",
            "route_kind": "cli",
            "execution_path": "local_execution_runtime_miss",
            "local_execution_runtime_miss_reason": "all_candidates_skipped"
        })),
    );

    assert_eq!(
        snapshot,
        UsageRoutingSnapshot {
            candidate_id: Some("cand-1".to_string()),
            candidate_index: None,
            key_name: Some("primary".to_string()),
            planner_kind: Some("claude_cli_sync".to_string()),
            route_family: Some("claude".to_string()),
            route_kind: Some("cli".to_string()),
            execution_path: Some("local_execution_runtime_miss".to_string()),
            local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
            selected_provider_id: Some("provider-1".to_string()),
            selected_endpoint_id: Some("endpoint-1".to_string()),
            selected_provider_api_key_id: Some("provider-key-1".to_string()),
            has_format_conversion: Some(false),
        }
    );

    let empty_snapshot = usage_routing_snapshot_from_usage(
        &UpsertUsageRecord {
            request_id: "req-124".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "openai".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: Some("provider-2".to_string()),
            provider_endpoint_id: Some("endpoint-2".to_string()),
            provider_api_key_id: Some("provider-key-2".to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(true),
            is_stream: Some(false),
            input_tokens: Some(1),
            output_tokens: Some(2),
            total_tokens: Some(3),
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
            response_time_ms: Some(100),
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
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 100,
        },
        Some(&json!({"trace_id": "trace-1"})),
    );

    assert_eq!(empty_snapshot, UsageRoutingSnapshot::default());
}

#[test]
fn usage_routing_snapshot_from_usage_prefers_typed_routing_fields_without_metadata() {
    let snapshot = usage_routing_snapshot_from_usage(
        &UpsertUsageRecord {
            request_id: "req-typed-routing-1".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "openai".to_string(),
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
            input_tokens: Some(1),
            output_tokens: Some(2),
            total_tokens: Some(3),
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
            response_time_ms: Some(100),
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
            updated_at_unix_secs: 100,
        },
        None,
    );

    assert_eq!(
        snapshot,
        UsageRoutingSnapshot {
            candidate_id: Some("cand-typed".to_string()),
            candidate_index: Some(2),
            key_name: Some("primary".to_string()),
            planner_kind: Some("claude_cli_sync".to_string()),
            route_family: Some("claude".to_string()),
            route_kind: Some("cli".to_string()),
            execution_path: Some("local_execution_runtime_miss".to_string()),
            local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
            selected_provider_id: Some("provider-1".to_string()),
            selected_endpoint_id: Some("endpoint-1".to_string()),
            selected_provider_api_key_id: Some("provider-key-1".to_string()),
            has_format_conversion: Some(true),
        }
    );
}

#[test]
fn attach_usage_routing_snapshot_metadata_adds_missing_keys_without_overwriting_existing_values() {
    let metadata = attach_usage_routing_snapshot_metadata(
        Some(json!({
            "candidate_id": "cand-existing",
            "route_kind": "cli"
        })),
        &UsageRoutingSnapshot {
            candidate_id: Some("cand-1".to_string()),
            candidate_index: Some(2),
            key_name: Some("primary".to_string()),
            planner_kind: Some("claude_cli_sync".to_string()),
            route_family: Some("claude".to_string()),
            route_kind: Some("chat".to_string()),
            execution_path: Some("local_execution_runtime_miss".to_string()),
            local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
            selected_provider_id: None,
            selected_endpoint_id: None,
            selected_provider_api_key_id: None,
            has_format_conversion: None,
        },
    )
    .expect("metadata should remain");

    assert_eq!(
        metadata,
        json!({
            "candidate_id": "cand-existing",
            "key_name": "primary",
            "planner_kind": "claude_cli_sync",
            "route_family": "claude",
            "route_kind": "cli",
            "execution_path": "local_execution_runtime_miss",
            "local_execution_runtime_miss_reason": "all_candidates_skipped"
        })
    );
}

#[test]
fn usage_settlement_pricing_snapshot_from_usage_extracts_typed_billing_fields() {
    let snapshot = usage_settlement_pricing_snapshot_from_usage(
        &UpsertUsageRecord {
            request_id: "req-125".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            provider_name: "openai".to_string(),
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
            input_tokens: Some(1),
            output_tokens: Some(2),
            total_tokens: Some(3),
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: Some(15.0),
            total_cost_usd: None,
            actual_total_cost_usd: None,
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(100),
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
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 100,
        },
        Some(&json!({
            "rate_multiplier": 0.5,
            "is_free_tier": false,
            "billing_snapshot": {
                "schema_version": "2.0",
                "status": "complete",
                "resolved_variables": {
                    "input_price_per_1m": 3.0,
                    "output_price_per_1m": 15.0,
                    "cache_creation_price_per_1m": 3.75,
                    "cache_read_price_per_1m": 0.30,
                    "price_per_request": 0.02
                }
            }
        })),
    )
    .expect("snapshot should build");

    assert_eq!(
        snapshot,
        UsageSettlementPricingSnapshot {
            billing_status: Some("pending".to_string()),
            billing_snapshot_schema_version: Some("2.0".to_string()),
            billing_snapshot_status: Some("complete".to_string()),
            billing_input_tokens: Some(1),
            billing_effective_input_tokens: Some(1),
            billing_output_tokens: Some(2),
            billing_total_input_context: Some(1),
            rate_multiplier: Some(0.5),
            is_free_tier: Some(false),
            input_price_per_1m: Some(3.0),
            output_price_per_1m: Some(15.0),
            cache_creation_price_per_1m: Some(3.75),
            cache_read_price_per_1m: Some(0.30),
            price_per_request: Some(0.02),
            ..UsageSettlementPricingSnapshot::default()
        }
    );
}

#[test]
fn usage_settlement_pricing_snapshot_with_billing_status_only_is_still_persisted() {
    let snapshot = UsageSettlementPricingSnapshot {
        billing_status: Some("pending".to_string()),
        ..UsageSettlementPricingSnapshot::default()
    };

    assert!(snapshot.any_present());
}

#[test]
fn attach_usage_settlement_pricing_snapshot_metadata_adds_missing_values_without_overwriting() {
    let metadata = attach_usage_settlement_pricing_snapshot_metadata(
        Some(json!({
            "rate_multiplier": 1.0,
            "billing_snapshot_status": "complete"
        })),
        &UsageSettlementPricingSnapshot {
            billing_status: None,
            billing_snapshot_schema_version: Some("2.0".to_string()),
            billing_snapshot_status: Some("incomplete".to_string()),
            rate_multiplier: Some(0.5),
            is_free_tier: Some(false),
            input_price_per_1m: Some(3.0),
            output_price_per_1m: Some(15.0),
            cache_creation_price_per_1m: Some(3.75),
            cache_read_price_per_1m: Some(0.30),
            price_per_request: Some(0.02),
            ..UsageSettlementPricingSnapshot::default()
        },
    )
    .expect("metadata should remain");

    assert_eq!(
        metadata,
        json!({
            "rate_multiplier": 1.0,
            "billing_snapshot_status": "complete",
            "billing_snapshot_schema_version": "2.0",
            "is_free_tier": false,
            "input_price_per_1m": 3.0,
            "output_price_per_1m": 15.0,
            "cache_creation_price_per_1m": 3.75,
            "cache_read_price_per_1m": 0.30,
            "price_per_request": 0.02
        })
    );
}
