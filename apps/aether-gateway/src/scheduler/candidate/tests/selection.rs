use std::sync::Arc;
use std::time::Duration;

use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::quota::InMemoryProviderQuotaRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot;
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::json;

use crate::cache::SchedulerAffinityTarget;
use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::data::candidate_selection::MinimalCandidateSelectionRowSource;
use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

use super::super::runtime::should_skip_provider_quota;
use super::super::selection::{
    collect_selectable_candidates as collect_selectable_candidates_impl,
    collect_selectable_candidates_with_skip_reasons as collect_selectable_candidates_with_skip_reasons_impl,
    is_exact_all_skipped_by_auth_limit, select_minimal_candidate as select_candidate_impl,
};
use super::support::{sample_auth_snapshot, sample_key, sample_provider, sample_row};

async fn select_candidate(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &AppState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<Option<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    select_candidate_impl(
        selection_row_source,
        runtime_state,
        api_format,
        global_model_name,
        require_streaming,
        None,
        auth_snapshot,
        now_unix_secs,
    )
    .await
}

async fn collect_selectable_candidates(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &AppState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    collect_selectable_candidates_impl(
        selection_row_source,
        runtime_state,
        api_format,
        global_model_name,
        require_streaming,
        None,
        auth_snapshot,
        now_unix_secs,
    )
    .await
}

async fn collect_selectable_candidates_with_skip_reasons(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &AppState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<
    (
        Vec<SchedulerMinimalCandidateSelectionCandidate>,
        Vec<super::super::SchedulerSkippedCandidate>,
    ),
    GatewayError,
> {
    collect_selectable_candidates_with_skip_reasons_impl(
        selection_row_source,
        runtime_state,
        api_format,
        global_model_name,
        require_streaming,
        None,
        auth_snapshot,
        now_unix_secs,
    )
    .await
}

fn provider_key_concurrency_row(
    provider_id: &str,
    endpoint_id: &str,
    key_id: &str,
    key_name: &str,
    provider_priority: i32,
    key_priority: i32,
) -> StoredMinimalCandidateSelectionRow {
    let mut row = sample_row();
    row.provider_id = provider_id.to_string();
    row.provider_name = provider_id.to_string();
    row.endpoint_id = endpoint_id.to_string();
    row.key_id = key_id.to_string();
    row.key_name = key_name.to_string();
    row.provider_priority = provider_priority;
    row.key_internal_priority = key_priority;
    row.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": key_priority}));
    row
}

fn provider_key_with_concurrent_limit(
    key_id: &str,
    provider_id: &str,
    concurrent_limit: Option<i32>,
) -> StoredProviderCatalogKey {
    let mut key = sample_key(key_id, provider_id, Some(10));
    key.concurrent_limit = concurrent_limit;
    key
}

fn active_provider_key_candidate(
    candidate_id: &str,
    request_id: &str,
    provider_id: &str,
    endpoint_id: &str,
    key_id: &str,
    status: RequestCandidateStatus,
) -> StoredRequestCandidate {
    StoredRequestCandidate::new(
        candidate_id.to_string(),
        request_id.to_string(),
        None,
        None,
        None,
        None,
        0,
        0,
        Some(provider_id.to_string()),
        Some(endpoint_id.to_string()),
        Some(key_id.to_string()),
        status,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        95_000,
        Some(95_000),
        None,
    )
    .expect("candidate should build")
}

fn provider_key_concurrency_state(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    keys: Vec<StoredProviderCatalogKey>,
    request_candidates: Vec<StoredRequestCandidate>,
) -> AppState {
    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("test-provider-a", None),
            sample_provider("test-provider-b", None),
        ],
        Vec::new(),
        keys,
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(request_candidates));

    AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        )
}

#[test]
fn skips_only_exhausted_monthly_quota_provider() {
    let inactive = StoredProviderQuotaSnapshot::new(
        "provider-1".to_string(),
        "monthly_quota".to_string(),
        Some(10.0),
        1.0,
        Some(30),
        Some(1_000),
        None,
        false,
    )
    .expect("quota should build");
    assert!(!should_skip_provider_quota(&inactive, 2_000));

    let expired = StoredProviderQuotaSnapshot::new(
        "provider-1".to_string(),
        "monthly_quota".to_string(),
        Some(10.0),
        1.0,
        Some(30),
        Some(1_000),
        Some(1_500),
        true,
    )
    .expect("quota should build");
    assert!(!should_skip_provider_quota(&expired, 2_000));

    let exhausted = StoredProviderQuotaSnapshot::new(
        "provider-1".to_string(),
        "monthly_quota".to_string(),
        Some(10.0),
        10.0,
        Some(30),
        Some(1_000),
        None,
        true,
    )
    .expect("quota should build");
    assert!(should_skip_provider_quota(&exhausted, 2_000));

    let payg = StoredProviderQuotaSnapshot::new(
        "provider-1".to_string(),
        "pay_as_you_go".to_string(),
        None,
        10.0,
        None,
        None,
        None,
        true,
    )
    .expect("quota should build");
    assert!(!should_skip_provider_quota(&payg, 2_000));
}

#[tokio::test]
async fn selects_by_provider_priority_when_priority_mode_is_provider() {
    let mut provider_first = sample_row();
    provider_first.provider_id = "provider-a".to_string();
    provider_first.provider_name = "provider-a".to_string();
    provider_first.endpoint_id = "endpoint-a".to_string();
    provider_first.key_id = "key-a".to_string();
    provider_first.key_name = "alpha".to_string();
    provider_first.provider_priority = 0;
    provider_first.key_internal_priority = 20;
    provider_first.key_global_priority_by_format = Some(json!({"openai:chat": 10}));

    let mut global_key_first = sample_row();
    global_key_first.provider_id = "provider-b".to_string();
    global_key_first.provider_name = "provider-b".to_string();
    global_key_first.endpoint_id = "endpoint-b".to_string();
    global_key_first.key_id = "key-b".to_string();
    global_key_first.key_name = "beta".to_string();
    global_key_first.provider_priority = 10;
    global_key_first.key_internal_priority = 0;
    global_key_first.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        provider_first,
        global_key_first,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas)
                .with_system_config_values_for_tests(vec![(
                    "provider_priority_mode".to_string(),
                    json!("provider"),
                )]),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-a");
    assert_eq!(selected.key_id, "key-a");
}

#[tokio::test]
async fn selects_by_global_key_priority_when_priority_mode_is_global_key() {
    let mut provider_first = sample_row();
    provider_first.provider_id = "provider-a".to_string();
    provider_first.provider_name = "provider-a".to_string();
    provider_first.endpoint_id = "endpoint-a".to_string();
    provider_first.key_id = "key-a".to_string();
    provider_first.key_name = "alpha".to_string();
    provider_first.provider_priority = 0;
    provider_first.key_internal_priority = 20;
    provider_first.key_global_priority_by_format = Some(json!({"openai:chat": 10}));

    let mut global_key_first = sample_row();
    global_key_first.provider_id = "provider-b".to_string();
    global_key_first.provider_name = "provider-b".to_string();
    global_key_first.endpoint_id = "endpoint-b".to_string();
    global_key_first.key_id = "key-b".to_string();
    global_key_first.key_name = "beta".to_string();
    global_key_first.provider_priority = 10;
    global_key_first.key_internal_priority = 0;
    global_key_first.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        provider_first,
        global_key_first,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas)
                .with_system_config_values_for_tests(vec![(
                    "provider_priority_mode".to_string(),
                    json!("global_key"),
                )]),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}

#[tokio::test]
async fn scheduler_selection_prefers_required_capability_matches_before_priority_fallback() {
    let mut higher_priority_missing_capability = sample_row();
    higher_priority_missing_capability.provider_id = "provider-a".to_string();
    higher_priority_missing_capability.provider_name = "provider-a".to_string();
    higher_priority_missing_capability.endpoint_id = "endpoint-a".to_string();
    higher_priority_missing_capability.key_id = "key-a".to_string();
    higher_priority_missing_capability.key_name = "alpha".to_string();
    higher_priority_missing_capability.provider_priority = 0;
    higher_priority_missing_capability.key_internal_priority = 0;
    higher_priority_missing_capability.key_capabilities = Some(json!({"cache_1h": false}));

    let mut lower_priority_matching_capability = sample_row();
    lower_priority_matching_capability.provider_id = "provider-b".to_string();
    lower_priority_matching_capability.provider_name = "provider-b".to_string();
    lower_priority_matching_capability.endpoint_id = "endpoint-b".to_string();
    lower_priority_matching_capability.key_id = "key-b".to_string();
    lower_priority_matching_capability.key_name = "beta".to_string();
    lower_priority_matching_capability.provider_priority = 10;
    lower_priority_matching_capability.key_internal_priority = 10;
    lower_priority_matching_capability.key_capabilities = Some(json!({"cache_1h": true}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        higher_priority_missing_capability,
        lower_priority_matching_capability,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas),
        );
    let required_capabilities = json!({"cache_1h": true});

    let selected = select_candidate_impl(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&required_capabilities),
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}

#[tokio::test]
async fn fixed_order_ignores_cached_scheduler_affinity_promotion() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "provider-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.provider_priority = 0;
    first.key_internal_priority = 0;
    first.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "provider-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.provider_priority = 1;
    second.key_internal_priority = 0;
    second.key_global_priority_by_format = Some(json!({"openai:chat": 1}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas)
                .with_system_config_values_for_tests(vec![(
                    "scheduling_mode".to_string(),
                    json!("fixed_order"),
                )]),
        );

    let auth_snapshot = sample_auth_snapshot("affinity-key-1");
    state.scheduler_affinity_cache.insert(
        "scheduler_affinity:affinity-key-1:openai:chat:gpt-4.1".to_string(),
        SchedulerAffinityTarget {
            provider_id: "provider-b".to_string(),
            endpoint_id: "endpoint-b".to_string(),
            key_id: "key-b".to_string(),
        },
        Duration::from_secs(300),
        100,
    );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&auth_snapshot),
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-a");
    assert_eq!(selected.key_id, "key-a");
}

#[tokio::test]
async fn fixed_order_disables_same_priority_affinity_hash_tiebreaker() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "provider-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.provider_priority = 0;
    first.key_internal_priority = 0;
    first.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "provider-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.provider_priority = 1;
    second.key_internal_priority = 0;
    second.key_global_priority_by_format = Some(json!({"openai:chat": 1}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas)
                .with_system_config_values_for_tests(vec![(
                    "scheduling_mode".to_string(),
                    json!("fixed_order"),
                )]),
        );

    let auth_snapshot = sample_auth_snapshot("affinity-key-1");
    let selection = collect_selectable_candidates(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&auth_snapshot),
        100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selection.len(), 2);
    assert_eq!(selection[0].provider_id, "provider-a");
    assert_eq!(selection[1].provider_id, "provider-b");
}

#[tokio::test]
async fn cache_affinity_promotes_cached_scheduler_affinity_candidate_when_enabled() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "provider-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.provider_priority = 0;
    first.key_internal_priority = 0;
    first.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "provider-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.provider_priority = 0;
    second.key_internal_priority = 0;
    second.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas)
                .with_system_config_values_for_tests(vec![(
                    "scheduling_mode".to_string(),
                    json!("cache_affinity"),
                )]),
        );

    let auth_snapshot = sample_auth_snapshot("affinity-key-1");
    state.scheduler_affinity_cache.insert(
        "scheduler_affinity:affinity-key-1:openai:chat:gpt-4.1".to_string(),
        SchedulerAffinityTarget {
            provider_id: "provider-b".to_string(),
            endpoint_id: "endpoint-b".to_string(),
            key_id: "key-b".to_string(),
        },
        Duration::from_secs(300),
        100,
    );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&auth_snapshot),
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}

#[tokio::test]
async fn load_balance_rotates_same_priority_group_and_ignores_cached_affinity() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "provider-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.provider_priority = 0;
    first.key_internal_priority = 0;
    first.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "provider-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.provider_priority = 0;
    second.key_internal_priority = 0;
    second.key_global_priority_by_format = Some(json!({"openai:chat": 0}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas)
                .with_system_config_values_for_tests(vec![(
                    "scheduling_mode".to_string(),
                    json!("load_balance"),
                )]),
        );

    let auth_snapshot = sample_auth_snapshot("affinity-key-1");
    state.scheduler_affinity_cache.insert(
        "scheduler_affinity:affinity-key-1:openai:chat:gpt-4.1".to_string(),
        SchedulerAffinityTarget {
            provider_id: "provider-b".to_string(),
            endpoint_id: "endpoint-b".to_string(),
            key_id: "key-b".to_string(),
        },
        Duration::from_secs(300),
        100,
    );

    let first_pass = collect_selectable_candidates(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&auth_snapshot),
        100,
    )
    .await
    .expect("first pass should succeed");
    let second_pass = collect_selectable_candidates(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&auth_snapshot),
        101,
    )
    .await
    .expect("second pass should succeed");

    assert_eq!(first_pass.len(), 2);
    assert_eq!(second_pass.len(), 2);
    assert_ne!(first_pass[0].provider_id, second_pass[0].provider_id);
    assert!(
        first_pass[0].provider_id != "provider-b" || second_pass[0].provider_id != "provider-b"
    );
    assert_ne!(
        first_pass
            .iter()
            .map(|candidate| candidate.provider_id.as_str())
            .collect::<Vec<_>>(),
        second_pass
            .iter()
            .map(|candidate| candidate.provider_id.as_str())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn selects_next_candidate_when_first_provider_quota_is_exhausted() {
    let mut first = sample_row();
    first.provider_id = "provider-1".to_string();
    first.provider_name = "openai-primary".to_string();
    first.endpoint_id = "endpoint-1".to_string();
    first.key_id = "key-1".to_string();
    first.key_name = "primary".to_string();
    first.model_provider_model_name = "gpt-4.1-primary".to_string();
    first.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-4.1-primary".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
    }]);
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-2".to_string();
    second.provider_name = "openai-secondary".to_string();
    second.endpoint_id = "endpoint-2".to_string();
    second.key_id = "key-2".to_string();
    second.key_name = "secondary".to_string();
    second.model_provider_model_name = "gpt-4.1-secondary".to_string();
    second.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-4.1-secondary".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
    }]);
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![
        StoredProviderQuotaSnapshot::new(
            "provider-1".to_string(),
            "monthly_quota".to_string(),
            Some(10.0),
            10.0,
            Some(30),
            Some(1_000),
            None,
            true,
        )
        .expect("quota should build"),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        2_000,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-2");
    assert_eq!(selected.selected_provider_model_name, "gpt-4.1-secondary");
}

#[tokio::test]
async fn cooled_down_when_recent_failures_are_recorded_for_same_key() {
    let mut first = sample_row();
    first.provider_id = "provider-1".to_string();
    first.provider_name = "openai-primary".to_string();
    first.endpoint_id = "endpoint-1".to_string();
    first.key_id = "key-1".to_string();
    first.key_name = "primary".to_string();
    first.model_provider_model_name = "gpt-4.1-primary".to_string();
    first.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-4.1-primary".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
    }]);
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-2".to_string();
    second.provider_name = "openai-secondary".to_string();
    second.endpoint_id = "endpoint-2".to_string();
    second.key_id = "key-2".to_string();
    second.key_name = "secondary".to_string();
    second.model_provider_model_name = "gpt-4.1-secondary".to_string();
    second.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-4.1-secondary".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
    }]);
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Failed,
            None,
            false,
            Some(502),
            None,
            Some("upstream".to_string()),
            Some(100),
            None,
            None,
            None,
            95_000,
            Some(95_000),
            Some(95_000),
        )
        .expect("candidate should build"),
        StoredRequestCandidate::new(
            "cand-2".to_string(),
            "req-2".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Cancelled,
            None,
            false,
            Some(499),
            None,
            Some("cancelled".to_string()),
            Some(80),
            None,
            None,
            None,
            98_000,
            Some(98_000),
            Some(98_000),
        )
        .expect("candidate should build"),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_quota_and_request_candidates_for_tests(
                candidates,
                quotas,
                request_candidates,
            ),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-2");
    assert_eq!(selected.selected_provider_model_name, "gpt-4.1-secondary");
}

#[tokio::test]
async fn selects_next_candidate_when_first_provider_concurrent_limit_is_reached() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "openai-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "openai-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-a", Some(1)),
            sample_provider("provider-b", None),
        ],
        Vec::new(),
        Vec::new(),
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-a".to_string()),
            Some("endpoint-a".to_string()),
            Some("key-a".to_string()),
            RequestCandidateStatus::Streaming,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            95_000,
            Some(95_000),
            None,
        )
        .expect("candidate should build"),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}

#[tokio::test]
async fn provider_key_concurrency_selects_next_key_when_first_provider_key_concurrent_limit_is_reached(
) {
    let state = provider_key_concurrency_state(
        vec![
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-a",
                "alpha",
                0,
                0,
            ),
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-b",
                "beta",
                0,
                1,
            ),
        ],
        vec![
            provider_key_with_concurrent_limit("provider-key-a", "test-provider-a", Some(1)),
            provider_key_with_concurrent_limit("provider-key-b", "test-provider-a", Some(1)),
        ],
        vec![active_provider_key_candidate(
            "cand-provider-key-a",
            "req-provider-key-a",
            "test-provider-a",
            "endpoint-a",
            "provider-key-a",
            RequestCandidateStatus::Streaming,
        )],
    );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "test-provider-a");
    assert_eq!(selected.key_id, "provider-key-b");
}

#[tokio::test]
async fn provider_key_concurrency_selects_next_provider_when_all_provider_keys_concurrent_limit_reached(
) {
    let state = provider_key_concurrency_state(
        vec![
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-a",
                "alpha",
                0,
                0,
            ),
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-b",
                "beta",
                0,
                1,
            ),
            provider_key_concurrency_row(
                "test-provider-b",
                "endpoint-b",
                "provider-key-c",
                "gamma",
                1,
                0,
            ),
        ],
        vec![
            provider_key_with_concurrent_limit("provider-key-a", "test-provider-a", Some(1)),
            provider_key_with_concurrent_limit("provider-key-b", "test-provider-a", Some(1)),
            provider_key_with_concurrent_limit("provider-key-c", "test-provider-b", Some(1)),
        ],
        vec![
            active_provider_key_candidate(
                "cand-provider-key-a",
                "req-provider-key-a",
                "test-provider-a",
                "endpoint-a",
                "provider-key-a",
                RequestCandidateStatus::Pending,
            ),
            active_provider_key_candidate(
                "cand-provider-key-b",
                "req-provider-key-b",
                "test-provider-a",
                "endpoint-a",
                "provider-key-b",
                RequestCandidateStatus::Streaming,
            ),
        ],
    );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "test-provider-b");
    assert_eq!(selected.key_id, "provider-key-c");
}

#[tokio::test]
async fn provider_key_concurrency_returns_none_when_all_provider_keys_concurrent_limit_reached() {
    let state = provider_key_concurrency_state(
        vec![
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-a",
                "alpha",
                0,
                0,
            ),
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-b",
                "beta",
                0,
                1,
            ),
            provider_key_concurrency_row(
                "test-provider-b",
                "endpoint-b",
                "provider-key-c",
                "gamma",
                1,
                0,
            ),
        ],
        vec![
            provider_key_with_concurrent_limit("provider-key-a", "test-provider-a", Some(1)),
            provider_key_with_concurrent_limit("provider-key-b", "test-provider-a", Some(1)),
            provider_key_with_concurrent_limit("provider-key-c", "test-provider-b", Some(1)),
        ],
        vec![
            active_provider_key_candidate(
                "cand-provider-key-a",
                "req-provider-key-a",
                "test-provider-a",
                "endpoint-a",
                "provider-key-a",
                RequestCandidateStatus::Pending,
            ),
            active_provider_key_candidate(
                "cand-provider-key-b",
                "req-provider-key-b",
                "test-provider-a",
                "endpoint-a",
                "provider-key-b",
                RequestCandidateStatus::Streaming,
            ),
            active_provider_key_candidate(
                "cand-provider-key-c",
                "req-provider-key-c",
                "test-provider-b",
                "endpoint-b",
                "provider-key-c",
                RequestCandidateStatus::Streaming,
            ),
        ],
    );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed");

    assert!(selected.is_none());

    let (selected_candidates, skipped_candidates) =
        collect_selectable_candidates_with_skip_reasons(
            state.data.as_ref(),
            &state,
            "openai:chat",
            "gpt-4.1",
            false,
            None,
            100,
        )
        .await
        .expect("selection should succeed");

    assert!(selected_candidates.is_empty());
    assert_eq!(skipped_candidates.len(), 3);
    assert!(skipped_candidates
        .iter()
        .all(|skipped| { skipped.skip_reason == "provider_key_concurrency_limit_reached" }));
    assert!(!is_exact_all_skipped_by_auth_limit(
        &selected_candidates,
        &skipped_candidates,
    ));
}

#[tokio::test]
async fn provider_key_concurrency_collects_exact_skip_reason_for_saturated_provider_keys() {
    let state = provider_key_concurrency_state(
        vec![
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-a",
                "alpha",
                0,
                0,
            ),
            provider_key_concurrency_row(
                "test-provider-a",
                "endpoint-a",
                "provider-key-b",
                "beta",
                0,
                1,
            ),
        ],
        vec![
            provider_key_with_concurrent_limit("provider-key-a", "test-provider-a", Some(1)),
            provider_key_with_concurrent_limit("provider-key-b", "test-provider-a", Some(1)),
        ],
        vec![active_provider_key_candidate(
            "cand-provider-key-a",
            "req-provider-key-a",
            "test-provider-a",
            "endpoint-a",
            "provider-key-a",
            RequestCandidateStatus::Pending,
        )],
    );

    let (selected_candidates, skipped_candidates) =
        collect_selectable_candidates_with_skip_reasons(
            state.data.as_ref(),
            &state,
            "openai:chat",
            "gpt-4.1",
            false,
            None,
            100,
        )
        .await
        .expect("selection should succeed");

    assert_eq!(selected_candidates.len(), 1);
    assert_eq!(selected_candidates[0].provider_id, "test-provider-a");
    assert_eq!(selected_candidates[0].key_id, "provider-key-b");
    assert_eq!(skipped_candidates.len(), 1);
    assert_eq!(
        skipped_candidates[0].candidate.provider_id,
        "test-provider-a"
    );
    assert_eq!(skipped_candidates[0].candidate.key_id, "provider-key-a");
    assert_eq!(
        skipped_candidates[0].skip_reason,
        "provider_key_concurrency_limit_reached",
    );
}

#[tokio::test]
async fn returns_none_when_auth_api_key_concurrent_limit_is_reached() {
    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        sample_row(),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", None)],
        Vec::new(),
        Vec::new(),
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Pending,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            95_000,
            Some(95_000),
            None,
        )
        .expect("candidate should build"),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let mut auth_snapshot = sample_auth_snapshot("api-key-1");
    auth_snapshot.api_key_concurrent_limit = Some(1);

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&auth_snapshot),
        100,
    )
    .await
    .expect("selection should succeed");

    assert!(selected.is_none());

    let (selected_candidates, skipped_candidates) =
        collect_selectable_candidates_with_skip_reasons(
            state.data.as_ref(),
            &state,
            "openai:chat",
            "gpt-4.1",
            false,
            Some(&auth_snapshot),
            100,
        )
        .await
        .expect("selection should succeed");
    assert!(is_exact_all_skipped_by_auth_limit(
        &selected_candidates,
        &skipped_candidates,
    ));
}

#[tokio::test]
async fn selects_next_candidate_when_first_provider_key_rpm_slots_are_reserved_for_new_user() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "openai-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "openai-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-a", None),
            sample_provider("provider-b", None),
        ],
        Vec::new(),
        vec![
            sample_key("key-a", "provider-a", Some(10)),
            sample_key("key-b", "provider-b", Some(10)),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            None,
            Some("api-key-new-user".to_string()),
            None,
            None,
            0,
            0,
            Some("provider-a".to_string()),
            Some("endpoint-a".to_string()),
            Some("key-a".to_string()),
            RequestCandidateStatus::Success,
            None,
            false,
            Some(200),
            None,
            None,
            Some(10),
            Some(9),
            None,
            None,
            95_000,
            Some(95_000),
            Some(96_000),
        )
        .expect("candidate should build"),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let auth_snapshot = sample_auth_snapshot("api-key-new-user");

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        Some(&auth_snapshot),
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}

#[tokio::test]
async fn selects_next_candidate_when_first_provider_key_circuit_is_open() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "openai-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "openai-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-a", None),
            sample_provider("provider-b", None),
        ],
        Vec::new(),
        vec![
            sample_key("key-a", "provider-a", Some(10)).with_health_fields(
                Some(serde_json::json!({"openai:chat": {"health_score": 0.2}})),
                Some(serde_json::json!({"openai:chat": {"open": true}})),
            ),
            sample_key("key-b", "provider-b", Some(10)),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}

#[tokio::test]
async fn exposes_runtime_skipped_candidates_with_skip_reasons() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "openai-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "openai-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-a", None),
            sample_provider("provider-b", None),
        ],
        Vec::new(),
        vec![
            sample_key("key-a", "provider-a", Some(10)).with_health_fields(
                Some(serde_json::json!({"openai:chat": {"health_score": 0.2}})),
                Some(serde_json::json!({"openai:chat": {"open": true}})),
            ),
            sample_key("key-b", "provider-b", Some(10)),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-b");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].candidate.provider_id, "provider-a");
    assert_eq!(skipped[0].skip_reason, "key_circuit_open");
    assert!(!is_exact_all_skipped_by_auth_limit(&selected, &skipped));
}

#[tokio::test]
async fn skips_codex_candidate_when_account_quota_is_exhausted_and_pool_flag_enabled() {
    let mut first = sample_row();
    first.provider_id = "provider-codex".to_string();
    first.provider_name = "codex".to_string();
    first.provider_type = "codex".to_string();
    first.endpoint_id = "endpoint-codex".to_string();
    first.endpoint_api_format = "openai:responses".to_string();
    first.key_id = "key-codex".to_string();
    first.key_name = "codex-exhausted".to_string();
    first.key_auth_type = "oauth".to_string();
    first.key_api_formats = Some(vec!["openai:responses".to_string()]);
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:responses": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-openai".to_string();
    second.provider_name = "openai".to_string();
    second.endpoint_id = "endpoint-openai".to_string();
    second.endpoint_api_format = "openai:responses".to_string();
    second.key_id = "key-openai".to_string();
    second.key_name = "fallback".to_string();
    second.key_api_formats = Some(vec!["openai:responses".to_string()]);
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:responses": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let mut codex_provider = sample_provider("provider-codex", None);
    codex_provider.provider_type = "codex".to_string();
    codex_provider.config = Some(serde_json::json!({
        "pool_advanced": {
            "skip_exhausted_accounts": true
        }
    }));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![codex_provider, sample_provider("provider-openai", None)],
        Vec::new(),
        vec![
            {
                let mut key = sample_key("key-codex", "provider-codex", Some(10));
                key.auth_type = "oauth".to_string();
                key.upstream_metadata = Some(serde_json::json!({
                    "codex": {
                        "secondary_used_percent": 100.0
                    }
                }));
                key
            },
            sample_key("key-openai", "provider-openai", Some(10)),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "openai:responses",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-openai");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].candidate.provider_id, "provider-codex");
    assert_eq!(skipped[0].skip_reason, "account_quota_exhausted");
}

#[tokio::test]
async fn keeps_refresh_failed_oauth_candidate_selectable_before_local_auth_resolution() {
    let mut first = sample_row();
    first.provider_id = "provider-codex".to_string();
    first.provider_name = "codex".to_string();
    first.provider_type = "codex".to_string();
    first.endpoint_id = "endpoint-codex".to_string();
    first.endpoint_api_format = "openai:responses".to_string();
    first.key_id = "key-codex".to_string();
    first.key_name = "codex-invalid".to_string();
    first.key_auth_type = "oauth".to_string();
    first.key_api_formats = Some(vec!["openai:responses".to_string()]);
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:responses": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-openai".to_string();
    second.provider_name = "openai".to_string();
    second.endpoint_id = "endpoint-openai".to_string();
    second.endpoint_api_format = "openai:responses".to_string();
    second.key_id = "key-openai".to_string();
    second.key_name = "fallback".to_string();
    second.key_api_formats = Some(vec!["openai:responses".to_string()]);
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:responses": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let mut codex_provider = sample_provider("provider-codex", None);
    codex_provider.provider_type = "codex".to_string();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![codex_provider, sample_provider("provider-openai", None)],
        Vec::new(),
        vec![
            {
                let mut key = sample_key("key-codex", "provider-codex", Some(10));
                key.auth_type = "oauth".to_string();
                key.oauth_invalid_at_unix_secs = Some(1_710_000_000);
                key.oauth_invalid_reason = Some(
                    "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 已被使用并轮换，请重新登录授权"
                        .to_string(),
                );
                key
            },
            sample_key("key-openai", "provider-openai", Some(10)),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "openai:responses",
        "gpt-4.1",
        false,
        None,
        1_710_000_100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].provider_id, "provider-codex");
    assert_eq!(selected[1].provider_id, "provider-openai");
    assert!(skipped.is_empty());
}

#[tokio::test]
async fn keeps_request_failed_oauth_candidate_selectable() {
    let mut row = sample_row();
    row.provider_id = "provider-codex".to_string();
    row.provider_name = "codex".to_string();
    row.provider_type = "codex".to_string();
    row.endpoint_id = "endpoint-codex".to_string();
    row.endpoint_api_format = "openai:responses".to_string();
    row.key_id = "key-codex".to_string();
    row.key_name = "codex-check-failed".to_string();
    row.key_auth_type = "oauth".to_string();
    row.key_api_formats = Some(vec!["openai:responses".to_string()]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let mut provider = sample_provider("provider-codex", None);
    provider.provider_type = "codex".to_string();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![{
            let mut key = sample_key("key-codex", "provider-codex", Some(10));
            key.auth_type = "oauth".to_string();
            key.oauth_invalid_at_unix_secs = Some(1_710_000_000);
            key.oauth_invalid_reason = Some("[REQUEST_FAILED] 账号状态检查失败".to_string());
            key
        }],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "openai:responses",
        "gpt-4.1",
        false,
        None,
        1_710_000_100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-codex");
    assert!(skipped.is_empty());
}

#[tokio::test]
async fn keeps_codex_candidate_selectable_when_oauth_token_is_expired() {
    let mut row = sample_row();
    row.provider_id = "provider-codex".to_string();
    row.provider_name = "codex".to_string();
    row.provider_type = "codex".to_string();
    row.endpoint_id = "endpoint-codex".to_string();
    row.endpoint_api_format = "openai:responses".to_string();
    row.key_id = "key-codex".to_string();
    row.key_name = "codex-token-expired".to_string();
    row.key_auth_type = "oauth".to_string();
    row.key_api_formats = Some(vec!["openai:responses".to_string()]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let mut provider = sample_provider("provider-codex", None);
    provider.provider_type = "codex".to_string();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![{
            let mut key = sample_key("key-codex", "provider-codex", Some(10));
            key.auth_type = "oauth".to_string();
            key.oauth_invalid_at_unix_secs = Some(1_710_000_000);
            key.oauth_invalid_reason = Some("Codex Token 无效或已过期".to_string());
            key
        }],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "openai:responses",
        "gpt-4.1",
        false,
        None,
        1_710_000_100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-codex");
    assert!(skipped.is_empty());
}

#[tokio::test]
async fn keeps_refreshable_kiro_candidate_selectable_with_runtime_oauth_invalid_marker() {
    let mut row = sample_row();
    row.provider_id = "provider-kiro".to_string();
    row.provider_name = "kiro".to_string();
    row.provider_type = "kiro".to_string();
    row.endpoint_id = "endpoint-kiro".to_string();
    row.endpoint_api_format = "claude:messages".to_string();
    row.key_id = "key-kiro".to_string();
    row.key_name = "kiro-refreshable".to_string();
    row.key_auth_type = "oauth".to_string();
    row.key_api_formats = Some(vec!["claude:messages".to_string()]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let mut provider = sample_provider("provider-kiro", None);
    provider.provider_type = "kiro".to_string();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![{
            let mut key = sample_key("key-kiro", "provider-kiro", Some(10));
            key.auth_type = "oauth".to_string();
            key.encrypted_auth_config = Some("encrypted-refreshable-session".to_string());
            key.oauth_invalid_at_unix_secs = Some(1_710_000_000);
            key.oauth_invalid_reason = Some("Kiro Token 无效或已过期".to_string());
            key
        }],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "claude:messages",
        "gpt-4.1",
        false,
        None,
        1_710_000_100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-kiro");
    assert!(skipped.is_empty());
}

#[tokio::test]
async fn keeps_refreshable_kiro_candidate_selectable_when_oauth_token_expired() {
    let mut row = sample_row();
    row.provider_id = "provider-kiro".to_string();
    row.provider_name = "kiro".to_string();
    row.provider_type = "kiro".to_string();
    row.endpoint_id = "endpoint-kiro".to_string();
    row.endpoint_api_format = "claude:messages".to_string();
    row.key_id = "key-kiro".to_string();
    row.key_name = "kiro-expired-refreshable".to_string();
    row.key_auth_type = "oauth".to_string();
    row.key_api_formats = Some(vec!["claude:messages".to_string()]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let mut provider = sample_provider("provider-kiro", None);
    provider.provider_type = "kiro".to_string();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![{
            let mut key = sample_key("key-kiro", "provider-kiro", Some(10));
            key.auth_type = "oauth".to_string();
            key.encrypted_auth_config = Some("encrypted-refreshable-session".to_string());
            key.expires_at_unix_secs = Some(1_710_000_000);
            key
        }],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "claude:messages",
        "gpt-4.1",
        false,
        None,
        1_710_000_100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-kiro");
    assert!(skipped.is_empty());
}

#[tokio::test]
async fn keeps_kiro_candidate_selectable_after_refresh_failure() {
    let mut row = sample_row();
    row.provider_id = "provider-kiro".to_string();
    row.provider_name = "kiro".to_string();
    row.provider_type = "kiro".to_string();
    row.endpoint_id = "endpoint-kiro".to_string();
    row.endpoint_api_format = "claude:messages".to_string();
    row.key_id = "key-kiro".to_string();
    row.key_name = "kiro-refresh-failed".to_string();
    row.key_auth_type = "oauth".to_string();
    row.key_api_formats = Some(vec!["claude:messages".to_string()]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let mut provider = sample_provider("provider-kiro", None);
    provider.provider_type = "kiro".to_string();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![{
            let mut key = sample_key("key-kiro", "provider-kiro", Some(10));
            key.auth_type = "oauth".to_string();
            key.encrypted_auth_config = Some("encrypted-refreshable-session".to_string());
            key.oauth_invalid_at_unix_secs = Some(1_710_000_000);
            key.oauth_invalid_reason = Some(
                "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 无效、已过期或已撤销，请重新登录授权"
                    .to_string(),
            );
            key
        }],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "claude:messages",
        "gpt-4.1",
        false,
        None,
        1_710_000_100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-kiro");
    assert!(skipped.is_empty());
}

#[tokio::test]
async fn skips_refreshable_kiro_candidate_when_oauth_marker_is_account_block() {
    let mut row = sample_row();
    row.provider_id = "provider-kiro".to_string();
    row.provider_name = "kiro".to_string();
    row.provider_type = "kiro".to_string();
    row.endpoint_id = "endpoint-kiro".to_string();
    row.endpoint_api_format = "claude:messages".to_string();
    row.key_id = "key-kiro".to_string();
    row.key_name = "kiro-account-blocked".to_string();
    row.key_auth_type = "oauth".to_string();
    row.key_api_formats = Some(vec!["claude:messages".to_string()]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let mut provider = sample_provider("provider-kiro", None);
    provider.provider_type = "kiro".to_string();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
        vec![{
            let mut key = sample_key("key-kiro", "provider-kiro", Some(10));
            key.auth_type = "oauth".to_string();
            key.encrypted_auth_config = Some("encrypted-refreshable-session".to_string());
            key.oauth_invalid_at_unix_secs = Some(1_710_000_000);
            key.oauth_invalid_reason = Some("账户已封禁: account banned".to_string());
            key
        }],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "claude:messages",
        "gpt-4.1",
        false,
        None,
        1_710_000_100,
    )
    .await
    .expect("selection should succeed");

    assert!(selected.is_empty());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].candidate.provider_id, "provider-kiro");
    assert_eq!(skipped[0].skip_reason, "oauth_invalid");
}

#[tokio::test]
async fn keeps_codex_candidate_selectable_when_exhausted_account_flag_is_disabled() {
    let mut first = sample_row();
    first.provider_id = "provider-codex".to_string();
    first.provider_name = "codex".to_string();
    first.provider_type = "codex".to_string();
    first.endpoint_id = "endpoint-codex".to_string();
    first.endpoint_api_format = "openai:responses".to_string();
    first.key_id = "key-codex".to_string();
    first.key_name = "codex-exhausted".to_string();
    first.key_auth_type = "oauth".to_string();
    first.key_api_formats = Some(vec!["openai:responses".to_string()]);
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:responses": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-openai".to_string();
    second.provider_name = "openai".to_string();
    second.endpoint_id = "endpoint-openai".to_string();
    second.endpoint_api_format = "openai:responses".to_string();
    second.key_id = "key-openai".to_string();
    second.key_name = "fallback".to_string();
    second.key_api_formats = Some(vec!["openai:responses".to_string()]);
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:responses": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let mut codex_provider = sample_provider("provider-codex", None);
    codex_provider.provider_type = "codex".to_string();
    codex_provider.config = Some(serde_json::json!({
        "pool_advanced": {}
    }));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![codex_provider, sample_provider("provider-openai", None)],
        Vec::new(),
        vec![
            {
                let mut key = sample_key("key-codex", "provider-codex", Some(10));
                key.auth_type = "oauth".to_string();
                key.upstream_metadata = Some(serde_json::json!({
                    "codex": {
                        "secondary_used_percent": 100.0
                    }
                }));
                key
            },
            sample_key("key-openai", "provider-openai", Some(10)),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "openai:responses",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 2);
    assert!(selected
        .iter()
        .any(|candidate| candidate.provider_id == "provider-codex"));
    assert!(skipped.is_empty());
}

#[tokio::test]
async fn skips_kiro_candidate_when_account_quota_is_exhausted_and_pool_flag_enabled() {
    let mut first = sample_row();
    first.provider_id = "provider-kiro".to_string();
    first.provider_name = "kiro".to_string();
    first.provider_type = "kiro".to_string();
    first.endpoint_id = "endpoint-kiro".to_string();
    first.endpoint_api_format = "claude:messages".to_string();
    first.key_id = "key-kiro".to_string();
    first.key_name = "kiro-exhausted".to_string();
    first.key_auth_type = "oauth".to_string();
    first.key_api_formats = Some(vec!["claude:messages".to_string()]);
    first.key_global_priority_by_format = Some(serde_json::json!({"claude:messages": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-openai".to_string();
    second.provider_name = "openai".to_string();
    second.endpoint_id = "endpoint-openai".to_string();
    second.endpoint_api_format = "claude:messages".to_string();
    second.key_id = "key-openai".to_string();
    second.key_name = "fallback".to_string();
    second.key_api_formats = Some(vec!["claude:messages".to_string()]);
    second.key_global_priority_by_format = Some(serde_json::json!({"claude:messages": 2}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let mut kiro_provider = sample_provider("provider-kiro", None);
    kiro_provider.provider_type = "kiro".to_string();
    kiro_provider.config = Some(serde_json::json!({
        "pool_advanced": {
            "skip_exhausted_accounts": true
        }
    }));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![kiro_provider, sample_provider("provider-openai", None)],
        Vec::new(),
        vec![
            {
                let mut key = sample_key("key-kiro", "provider-kiro", Some(10));
                key.auth_type = "oauth".to_string();
                key.upstream_metadata = Some(serde_json::json!({
                    "kiro": {
                        "remaining": 0
                    }
                }));
                key
            },
            sample_key("key-openai", "provider-openai", Some(10)),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let (selected, skipped) = collect_selectable_candidates_with_skip_reasons(
        state.data.as_ref(),
        &state,
        "claude:messages",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].provider_id, "provider-openai");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].candidate.provider_id, "provider-kiro");
    assert_eq!(skipped[0].skip_reason, "account_quota_exhausted");
}

#[tokio::test]
async fn same_priority_candidates_prefer_healthier_provider_key_before_id_order() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "openai-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "openai-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-a", None),
            sample_provider("provider-b", None),
        ],
        Vec::new(),
        vec![
            sample_key("key-a", "provider-a", Some(10)).with_health_fields(
                Some(serde_json::json!({"openai:chat": {"health_score": 0.30}})),
                None,
            ),
            sample_key("key-b", "provider-b", Some(10)).with_health_fields(
                Some(serde_json::json!({"openai:chat": {"health_score": 0.95}})),
                None,
            ),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}

#[tokio::test]
async fn same_priority_candidates_use_aggregate_health_score_when_api_format_specific_health_is_missing(
) {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "openai-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "openai-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-a", None),
            sample_provider("provider-b", None),
        ],
        Vec::new(),
        vec![
            sample_key("key-a", "provider-a", Some(10)).with_health_fields(
                Some(serde_json::json!({
                    "openai:responses": {"health_score": 0.40},
                    "claude:messages": {"health_score": 0.55}
                })),
                None,
            ),
            sample_key("key-b", "provider-b", Some(10)).with_health_fields(
                Some(serde_json::json!({
                    "openai:responses": {"health_score": 0.90},
                    "claude:messages": {"health_score": 0.92}
                })),
                None,
            ),
        ],
    ));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests(
                candidates,
                provider_catalog,
                quotas,
                request_candidates,
            ),
        );

    let selected = select_candidate(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "gpt-4.1",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed")
    .expect("candidate should exist");

    assert_eq!(selected.provider_id, "provider-b");
    assert_eq!(selected.key_id, "key-b");
}
