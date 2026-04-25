use chrono::{TimeZone, Utc};
use serde_json::json;

use super::{
    attach_compressed_body_refs, attach_usage_http_audit_body_refs,
    attach_usage_routing_snapshot_metadata, attach_usage_settlement_pricing_snapshot_metadata,
    inflate_usage_json_value, prepare_request_metadata_for_body_storage,
    prepare_usage_body_storage, resolved_read_usage_body_ref, resolved_write_usage_body_ref,
    split_dashboard_daily_aggregate_range, split_dashboard_hourly_aggregate_range, usage_body_ref,
    usage_http_audit_body_refs, usage_http_audit_capture_mode, usage_routing_snapshot_from_usage,
    usage_settlement_pricing_snapshot_from_usage, AggregateRangeSplit, SqlxUsageReadRepository,
    UsageHttpAuditRefs, UsageRoutingSnapshot, UsageSettlementPricingSnapshot,
    MAX_INLINE_USAGE_BODY_BYTES,
};
use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};
use crate::repository::usage::UpsertUsageRecord;
use aether_data_contracts::repository::usage::UsageBodyField;

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
    assert!(super::SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL.contains("GROUP BY api_key_id"));
    assert!(super::SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL.contains("ANY($1::TEXT[])"));
}

#[test]
fn usage_sql_summarizes_usage_by_provider_api_key_ids_in_database() {
    assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL.contains("FROM provider_api_keys"));
    assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL
        .contains("COALESCE(request_count, 0) > 0"));
    assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL.contains("ANY($1::TEXT[])"));
}

#[test]
fn usage_sql_serializes_request_id_upserts_before_reading_previous_usage() {
    assert!(super::LOCK_USAGE_REQUEST_ID_SQL.contains("pg_advisory_xact_lock"));
    assert!(super::LOCK_USAGE_REQUEST_ID_SQL.contains("hashtext($1)::BIGINT"));
    assert!(include_str!("mod.rs")
        .contains("lock_usage_request_id_in_tx(tx, &usage.request_id).await?;"));
}

#[test]
fn usage_sql_rebuild_matches_online_api_key_usage_semantics() {
    assert!(super::REBUILD_API_KEY_USAGE_STATS_SQL.contains("COUNT(*)::INTEGER"));
    assert!(super::REBUILD_API_KEY_USAGE_STATS_SQL.contains("COALESCE("));
    assert!(super::REBUILD_API_KEY_USAGE_STATS_SQL.contains("total_tokens,"));
    assert!(super::REBUILD_API_KEY_USAGE_STATS_SQL
        .contains("COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)"));
    assert!(super::REBUILD_API_KEY_USAGE_STATS_SQL.contains("AND BTRIM(api_key_id) <> ''"));
}

#[test]
fn usage_sql_rebuild_matches_online_provider_key_usage_semantics() {
    assert!(super::REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL
        .contains("NULLIF(BTRIM(error_message), '') IS NULL"));
    assert!(super::REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL.contains("COALESCE("));
    assert!(super::REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL.contains("total_tokens,"));
    assert!(super::REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL
        .contains("COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)"));
    assert!(super::REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL
        .contains("AND BTRIM(provider_api_key_id) <> ''"));
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
    assert!(
        source.contains("split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc)")
    );
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
fn usage_sql_summarize_total_tokens_by_api_key_ids_supports_daily_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("FROM stats_daily_api_key"));
    assert!(source.contains("read_stats_daily_cutoff_date().await?"));
}

#[test]
fn usage_sql_summarize_usage_totals_by_user_ids_supports_user_summary_aggregates() {
    let source = include_str!("mod.rs");
    assert!(source.contains("FROM stats_user_summary"));
    assert!(source.contains("all_time_input_tokens"));
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
    for sql in [super::FIND_BY_REQUEST_ID_SQL, super::FIND_BY_ID_SQL] {
        assert!(sql.contains("CAST(\"usage\".input_tokens AS INTEGER) AS input_tokens"));
        assert!(sql.contains(
            "usage_settlement_snapshots.billing_input_tokens AS settlement_billing_input_tokens"
        ));
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
fn usage_sql_updates_usage_mirror_columns_from_terminal_events_only() {
    for assignment in [
        "input_tokens = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".input_tokens, EXCLUDED.input_tokens) ELSE \"usage\".input_tokens END",
        "output_tokens = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".output_tokens, EXCLUDED.output_tokens) ELSE \"usage\".output_tokens END",
        "total_tokens = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".total_tokens, EXCLUDED.total_tokens) ELSE \"usage\".total_tokens END",
        "cache_creation_input_tokens = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".cache_creation_input_tokens, EXCLUDED.cache_creation_input_tokens) ELSE \"usage\".cache_creation_input_tokens END",
        "cache_creation_input_tokens_5m = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".cache_creation_input_tokens_5m, EXCLUDED.cache_creation_input_tokens_5m) ELSE \"usage\".cache_creation_input_tokens_5m END",
        "cache_creation_input_tokens_1h = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".cache_creation_input_tokens_1h, EXCLUDED.cache_creation_input_tokens_1h) ELSE \"usage\".cache_creation_input_tokens_1h END",
        "cache_read_input_tokens = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".cache_read_input_tokens, EXCLUDED.cache_read_input_tokens) ELSE \"usage\".cache_read_input_tokens END",
        "cache_creation_cost_usd = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".cache_creation_cost_usd, EXCLUDED.cache_creation_cost_usd) ELSE \"usage\".cache_creation_cost_usd END",
        "cache_read_cost_usd = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".cache_read_cost_usd, EXCLUDED.cache_read_cost_usd) ELSE \"usage\".cache_read_cost_usd END",
        "total_cost_usd = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".total_cost_usd, EXCLUDED.total_cost_usd) ELSE \"usage\".total_cost_usd END",
        "actual_total_cost_usd = CASE WHEN \"usage\".billing_status = 'pending' AND EXCLUDED.status IN ('completed', 'failed', 'cancelled') THEN GREATEST(\"usage\".actual_total_cost_usd, EXCLUDED.actual_total_cost_usd) ELSE \"usage\".actual_total_cost_usd END",
    ] {
        assert!(
            super::UPSERT_SQL.contains(assignment),
            "missing terminal mirror assignment: {assignment}"
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
        .contains("WHEN EXCLUDED.request_body_compressed IS NOT NULL OR $56 THEN NULL"));
    assert!(super::UPSERT_SQL
        .contains("WHEN EXCLUDED.provider_request_body_compressed IS NOT NULL OR $57 THEN NULL"));
    assert!(super::UPSERT_SQL
        .contains("WHEN EXCLUDED.response_body_compressed IS NOT NULL OR $58 THEN NULL"));
    assert!(super::UPSERT_SQL
        .contains("WHEN EXCLUDED.client_response_body_compressed IS NOT NULL OR $59 THEN NULL"));
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
fn usage_sql_does_not_allow_streaming_to_regress_back_to_pending() {
    assert!(super::UPSERT_SQL.contains(
            "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'pending' THEN \"usage\".status_code"
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
