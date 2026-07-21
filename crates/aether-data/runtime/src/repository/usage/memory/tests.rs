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
    UsageAuditAggregationQuery, UsageBodyCaptureState, UsageBodyField, UsageDashboardSummaryQuery,
    UsageLeaderboardGroupBy, UsageLeaderboardQuery, UsageProviderPerformanceQuery,
    UsageTimeSeriesGranularity,
};
use serde_json::json;

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
async fn upsert_writes_usage_record() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
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
    assert_eq!(
        stored.request_metadata,
        Some(json!({
            "request_body_ref": "usage://request/req-upsert-body-ref-metadata/request_body"
        }))
    );
}

#[tokio::test]
async fn upsert_keeps_typed_routing_fields_out_of_request_metadata() {
    let repository = InMemoryUsageReadRepository::default();
    let stored = repository
        .upsert(UpsertUsageRecord {
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
            created_from_unix_secs: 1_711_000_000,
            created_until_unix_secs: 1_711_000_001,
            group_by: UsageLeaderboardGroupBy::User,
            user_id: None,
            provider_name: None,
            model: None,
        })
        .await
        .expect("leaderboard should summarize");
    assert_eq!(leaderboard.len(), 1);
    assert_eq!(leaderboard[0].total_tokens, 120);
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
