use std::sync::Arc;
use std::time::Duration;

use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::quota::InMemoryProviderQuotaRepository;
use aether_data_contracts::repository::candidate_selection::StoredProviderModelMapping;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_scheduler_core::{
    apply_scheduler_candidate_ranking, candidate_affinity_hash,
    enumerate_minimal_candidate_selection, EnumerateMinimalCandidateSelectionInput,
    SchedulerMinimalCandidateSelectionCandidate, SchedulerPriorityMode, SchedulerRankableCandidate,
    SchedulerRankingContext, SchedulerRankingMode,
};

use crate::cache::SchedulerAffinityTarget;
use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::data::candidate_selection::{
    read_requested_model_rows, MinimalCandidateSelectionRowSource,
};
use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

use super::super::affinity::build_scheduler_affinity_cache_key;
use super::super::selection::select_minimal_candidate as select_candidate_impl;
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

#[tokio::test]
async fn same_priority_candidates_are_distributed_by_affinity_key() {
    let mut first = sample_row();
    first.provider_id = "provider-a".to_string();
    first.provider_name = "openai-a".to_string();
    first.endpoint_id = "endpoint-a".to_string();
    first.key_id = "key-a".to_string();
    first.key_name = "alpha".to_string();
    first.provider_priority = 10;
    first.key_internal_priority = 10;
    first.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));
    first.model_provider_model_name = "gpt-4.1-a".to_string();
    first.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-4.1-a".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
    }]);

    let mut second = sample_row();
    second.provider_id = "provider-b".to_string();
    second.provider_name = "openai-b".to_string();
    second.endpoint_id = "endpoint-b".to_string();
    second.key_id = "key-b".to_string();
    second.key_name = "beta".to_string();
    second.provider_priority = 10;
    second.key_internal_priority = 10;
    second.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 1}));
    second.model_provider_model_name = "gpt-4.1-b".to_string();
    second.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-4.1-b".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
    }]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        first, second,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas);
    let auth_snapshot = sample_auth_snapshot("affinity-key-1");

    let (_resolved_global_model_name, rows) =
        read_requested_model_rows(&state, "openai:chat", "gpt-4.1")
            .await
            .expect("selection rows should read")
            .expect("selection rows should match requested model");
    let mut selection =
        enumerate_minimal_candidate_selection(EnumerateMinimalCandidateSelectionInput {
            rows,
            normalized_api_format: "openai:chat",
            requested_model_name: "gpt-4.1",
            resolved_global_model_name: "gpt-4.1",
            require_streaming: false,
            required_capabilities: None,
            auth_constraints: None,
        })
        .expect("selection should succeed");
    let rankables = selection
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            SchedulerRankableCandidate::from_candidate(candidate, index).with_affinity_hash(Some(
                candidate_affinity_hash(auth_snapshot.api_key_id.as_str(), candidate),
            ))
        })
        .collect::<Vec<_>>();
    apply_scheduler_candidate_ranking(
        &mut selection,
        &rankables,
        SchedulerRankingContext {
            priority_mode: SchedulerPriorityMode::Provider,
            ranking_mode: SchedulerRankingMode::CacheAffinity,
            include_health: false,
            load_balance_seed: 0,
        },
    );

    assert_eq!(selection.len(), 2);

    let left_hash = candidate_affinity_hash(&auth_snapshot.api_key_id, &selection[0]);
    let right_hash = candidate_affinity_hash(&auth_snapshot.api_key_id, &selection[1]);
    assert!(
        left_hash <= right_hash,
        "same-priority candidates should be ordered by affinity hash"
    );
}

#[tokio::test]
async fn reuses_cached_scheduler_affinity_candidate_before_sorted_fallback() {
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
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas),
        );

    let auth_snapshot = sample_auth_snapshot("affinity-key-1");
    let cache_key =
        build_scheduler_affinity_cache_key(Some(&auth_snapshot), "openai:chat", "gpt-4.1")
            .expect("cache key should build");
    state.scheduler_affinity_cache.insert(
        cache_key,
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
async fn cached_affinity_candidate_can_use_reserved_provider_key_rpm_capacity() {
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
            Some("api-key-cached-user".to_string()),
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

    let auth_snapshot = sample_auth_snapshot("api-key-cached-user");
    let cache_key =
        build_scheduler_affinity_cache_key(Some(&auth_snapshot), "openai:chat", "gpt-4.1")
            .expect("cache key should build");
    state.scheduler_affinity_cache.insert(
        cache_key,
        SchedulerAffinityTarget {
            provider_id: "provider-a".to_string(),
            endpoint_id: "endpoint-a".to_string(),
            key_id: "key-a".to_string(),
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
