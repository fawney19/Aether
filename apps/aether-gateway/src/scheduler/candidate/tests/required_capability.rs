use std::sync::Arc;

use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::quota::InMemoryProviderQuotaRepository;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};

use crate::data::GatewayDataState;
use crate::AppState;

use super::super::{
    list_selectable_candidates_for_required_capability_without_requested_model,
    list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal,
};
use super::support::{sample_auth_snapshot, sample_provider, sample_row};

#[tokio::test]
async fn compatible_required_capability_prefers_matching_keys_without_hard_filtering() {
    let mut higher_priority = sample_row();
    higher_priority.provider_id = "provider-a".to_string();
    higher_priority.provider_name = "provider-a".to_string();
    higher_priority.endpoint_id = "endpoint-a".to_string();
    higher_priority.key_id = "key-a".to_string();
    higher_priority.key_name = "alpha".to_string();
    higher_priority.provider_priority = 0;
    higher_priority.key_internal_priority = 0;
    higher_priority.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 0}));
    higher_priority.key_capabilities = Some(serde_json::json!({}));

    let mut capability_match = sample_row();
    capability_match.provider_id = "provider-b".to_string();
    capability_match.provider_name = "provider-b".to_string();
    capability_match.endpoint_id = "endpoint-b".to_string();
    capability_match.key_id = "key-b".to_string();
    capability_match.key_name = "beta".to_string();
    capability_match.provider_priority = 10;
    capability_match.key_internal_priority = 10;
    capability_match.key_global_priority_by_format = Some(serde_json::json!({"openai:chat": 10}));
    capability_match.key_capabilities = Some(serde_json::json!({"cache_1h": true}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        higher_priority,
        capability_match,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas),
        );

    let selection = list_selectable_candidates_for_required_capability_without_requested_model(
        state.data.as_ref(),
        &state,
        "openai:chat",
        "cache_1h",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selection.len(), 2);
    assert_eq!(selection[0].provider_id, "provider-b");
    assert_eq!(selection[1].provider_id, "provider-a");
}

#[tokio::test]
async fn exclusive_required_capability_keeps_hard_filtering_only_matching_keys() {
    let mut incompatible = sample_row();
    incompatible.provider_id = "provider-a".to_string();
    incompatible.provider_name = "provider-a".to_string();
    incompatible.endpoint_id = "endpoint-a".to_string();
    incompatible.endpoint_api_format = "gemini:chat".to_string();
    incompatible.endpoint_api_family = Some("gemini".to_string());
    incompatible.key_api_formats = Some(vec!["gemini:chat".to_string()]);
    incompatible.key_id = "key-a".to_string();
    incompatible.key_name = "alpha".to_string();
    incompatible.global_model_name = "gemini-2.5-pro".to_string();
    incompatible.key_capabilities = Some(serde_json::json!({}));

    let mut compatible = sample_row();
    compatible.provider_id = "provider-b".to_string();
    compatible.provider_name = "provider-b".to_string();
    compatible.endpoint_id = "endpoint-b".to_string();
    compatible.endpoint_api_format = "gemini:chat".to_string();
    compatible.endpoint_api_family = Some("gemini".to_string());
    compatible.key_api_formats = Some(vec!["gemini:chat".to_string()]);
    compatible.key_id = "key-b".to_string();
    compatible.key_name = "beta".to_string();
    compatible.global_model_name = "gemini-2.5-pro".to_string();
    compatible.key_capabilities = Some(serde_json::json!({"gemini_files": true}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        incompatible,
        compatible,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas),
        );

    let selection = list_selectable_candidates_for_required_capability_without_requested_model(
        state.data.as_ref(),
        &state,
        "gemini:chat",
        "gemini_files",
        false,
        None,
        100,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selection.len(), 1);
    assert_eq!(selection[0].provider_id, "provider-b");
    assert_eq!(selection[0].key_id, "key-b");
}

#[tokio::test]
async fn required_capability_reports_auth_limit_signal_when_every_model_is_blocked_by_api_key_concurrency(
) {
    let mut candidate = sample_row();
    candidate.key_capabilities = Some(serde_json::json!({"cache_1h": true}));

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        candidate,
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

    let (selection, auth_limit_blocked) =
        list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal(
            state.data.as_ref(),
            &state,
            "openai:chat",
            "cache_1h",
            false,
            Some(&auth_snapshot),
            100,
        )
        .await
        .expect("selection should succeed");

    assert!(selection.is_empty());
    assert!(auth_limit_blocked);
}
