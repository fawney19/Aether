use std::sync::Arc;

use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::quota::InMemoryProviderQuotaRepository;
use aether_data_contracts::repository::candidate_selection::StoredProviderModelMapping;
use aether_scheduler_core::{
    resolve_requested_global_model_name, SchedulerMinimalCandidateSelectionCandidate,
};

use crate::data::candidate_selection::enumerate_minimal_candidate_selection_with_required_capabilities;
use crate::data::GatewayDataState;

use super::super::{
    candidate_model_names, matches_model_mapping, resolve_provider_model_name,
    select_provider_model_name,
};
use super::support::{sample_auth_snapshot, sample_row};

#[test]
fn selects_provider_model_name_with_api_format_scope() {
    let row = sample_row();

    assert_eq!(
        select_provider_model_name(&row, "openai:chat"),
        "gpt-4.1-canary"
    );
}

#[test]
fn candidate_model_names_keep_base_and_scoped_mappings() {
    let row = sample_row();
    let names = candidate_model_names(&row, "openai:chat");

    assert!(names.contains("gpt-4.1-upstream"));
    assert!(names.contains("gpt-4.1-canary"));
    assert!(!names.contains("gpt-4.1-responses"));
}

#[test]
fn provider_model_mapping_respects_endpoint_scope() {
    let mut row = sample_row();
    row.endpoint_id = "endpoint-selected".to_string();
    row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-4.1-endpoint".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
        endpoint_ids: Some(vec!["endpoint-selected".to_string()]),
        operations: None,
    }]);

    assert_eq!(
        select_provider_model_name(&row, "openai:chat"),
        "gpt-4.1-endpoint"
    );
    assert!(candidate_model_names(&row, "openai:chat").contains("gpt-4.1-endpoint"));

    row.endpoint_id = "endpoint-other".to_string();

    assert_eq!(
        select_provider_model_name(&row, "openai:chat"),
        "gpt-4.1-upstream"
    );
    assert!(!candidate_model_names(&row, "openai:chat").contains("gpt-4.1-endpoint"));
}

#[test]
fn resolves_mapping_matched_model_from_key_allowed_models() {
    let mut row = sample_row();
    row.key_allowed_models = Some(vec!["gpt-4.1-upstream".to_string()]);

    let resolved = resolve_provider_model_name(&row, "gpt-4.1", "openai:chat")
        .expect("candidate should resolve");

    assert_eq!(resolved.0, "gpt-4.1-canary");
    assert_eq!(resolved.1, Some("gpt-4.1-upstream".to_string()));
}

#[test]
fn resolves_mapping_matched_model_from_global_regex_mapping() {
    let mut row = sample_row();
    row.key_allowed_models = Some(vec!["gpt-4.1-variant".to_string()]);

    let resolved = resolve_provider_model_name(&row, "gpt-4.1", "openai:chat")
        .expect("candidate should resolve");

    assert_eq!(resolved.0, "gpt-4.1-variant");
    assert_eq!(resolved.1, Some("gpt-4.1-variant".to_string()));
}

#[test]
fn invalid_regex_mapping_is_treated_as_non_match() {
    assert!(!matches_model_mapping("(", "gpt-4.1-variant"));
}

#[test]
fn resolves_requested_global_model_from_provider_model_alias() {
    let mut row = sample_row();
    row.global_model_name = "gpt-5".to_string();
    row.model_provider_model_name = "gpt-5.2".to_string();
    row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-5.2".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
        endpoint_ids: None,
        operations: None,
    }]);

    let resolved = resolve_requested_global_model_name(&[row], "gpt-5.2", "openai:chat");

    assert_eq!(resolved.as_deref(), Some("gpt-5"));
}

#[test]
fn resolves_requested_global_model_from_global_regex_mapping() {
    let mut row = sample_row();
    row.global_model_name = "gpt-5".to_string();
    row.global_model_mappings = Some(vec!["gpt-5(?:\\.\\d+)?".to_string()]);

    let resolved = resolve_requested_global_model_name(&[row], "gpt-5.2", "openai:chat");

    assert_eq!(resolved.as_deref(), Some("gpt-5"));
}

#[test]
fn scheduler_candidate_is_serializable() {
    let candidate = SchedulerMinimalCandidateSelectionCandidate {
        provider_id: "provider-1".to_string(),
        provider_name: "OpenAI".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        endpoint_id: "endpoint-1".to_string(),
        endpoint_api_format: "openai:chat".to_string(),
        key_id: "key-1".to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_internal_priority: 50,
        key_global_priority_for_format: Some(2),
        key_capabilities: Some(serde_json::json!({"cache_1h": true})),
        model_id: "model-1".to_string(),
        global_model_id: "global-model-1".to_string(),
        global_model_name: "gpt-4.1".to_string(),
        selected_provider_model_name: "gpt-4.1-canary".to_string(),
        supports_streaming: true,
        mapping_matched_model: Some("gpt-4.1-canary".to_string()),
    };

    let json = serde_json::to_value(candidate).expect("candidate should serialize");
    assert_eq!(json["provider_name"], "OpenAI");
}

#[tokio::test]
async fn enumerate_minimal_candidate_selection_resolves_provider_model_alias() {
    let mut row = sample_row();
    row.global_model_name = "gpt-5".to_string();
    row.model_provider_model_name = "gpt-5.2".to_string();
    row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-5.2".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
        endpoint_ids: None,
        operations: None,
    }]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas);

    let selection = enumerate_minimal_candidate_selection_with_required_capabilities(
        &state,
        "openai:chat",
        "gpt-5.2",
        false,
        None,
        None,
        false,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selection.len(), 1);
    assert_eq!(selection[0].global_model_name, "gpt-5");
    assert_eq!(selection[0].selected_provider_model_name, "gpt-5.2");
}

#[tokio::test]
async fn enumerate_minimal_candidate_selection_filters_endpoint_scoped_alias_rows() {
    let mut selected = sample_row();
    selected.endpoint_id = "endpoint-selected".to_string();
    selected.key_id = "key-selected".to_string();
    selected.global_model_name = "gpt-5".to_string();
    selected.model_provider_model_name = "gpt-5-upstream".to_string();
    selected.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-5-alias".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
        endpoint_ids: Some(vec!["endpoint-selected".to_string()]),
        operations: None,
    }]);

    let mut other = selected.clone();
    other.endpoint_id = "endpoint-other".to_string();
    other.key_id = "key-other".to_string();

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        selected, other,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas);

    let selection = enumerate_minimal_candidate_selection_with_required_capabilities(
        &state,
        "openai:chat",
        "gpt-5-alias",
        false,
        None,
        None,
        false,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selection.len(), 1);
    assert_eq!(selection[0].endpoint_id, "endpoint-selected");
    assert_eq!(selection[0].selected_provider_model_name, "gpt-5-alias");
}

#[tokio::test]
async fn enumerate_minimal_candidate_selection_keeps_only_resolved_global_model_rows() {
    let mut exact = sample_row();
    exact.provider_id = "provider-exact".to_string();
    exact.endpoint_id = "endpoint-exact".to_string();
    exact.key_id = "key-exact".to_string();
    exact.model_id = "model-exact".to_string();
    exact.global_model_id = "global-exact".to_string();
    exact.global_model_name = "gpt-5".to_string();
    exact.model_provider_model_name = "gpt-5".to_string();
    exact.model_provider_model_mappings = None;

    let mut mapped = sample_row();
    mapped.provider_id = "provider-mapped".to_string();
    mapped.endpoint_id = "endpoint-mapped".to_string();
    mapped.key_id = "key-mapped".to_string();
    mapped.model_id = "model-mapped".to_string();
    mapped.global_model_id = "global-mapped".to_string();
    mapped.global_model_name = "claude-sonnet".to_string();
    mapped.global_model_mappings = Some(vec!["gpt-5".to_string()]);
    mapped.model_provider_model_name = "claude-sonnet-upstream".to_string();
    mapped.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "claude-sonnet-upstream".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
        endpoint_ids: None,
        operations: None,
    }]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        exact, mapped,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas);

    let selection = enumerate_minimal_candidate_selection_with_required_capabilities(
        &state,
        "openai:chat",
        "gpt-5",
        false,
        None,
        None,
        false,
    )
    .await
    .expect("selection should succeed");

    let provider_ids = selection
        .iter()
        .map(|candidate| candidate.provider_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(provider_ids, vec!["provider-exact"]);
    assert_eq!(selection[0].selected_provider_model_name, "gpt-5");
}

#[tokio::test]
async fn enumerate_minimal_candidate_selection_allows_resolved_global_model_in_auth_snapshot() {
    let mut row = sample_row();
    row.global_model_name = "gpt-5".to_string();
    row.global_model_mappings = Some(vec!["gpt-5(?:\\.\\d+)?".to_string()]);
    row.model_provider_model_name = "gpt-5-upstream".to_string();
    row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-5-upstream".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
        endpoint_ids: None,
        operations: None,
    }]);

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas);
    let mut auth_snapshot = sample_auth_snapshot("api-key-1");
    auth_snapshot.user_allowed_models = Some(vec!["gpt-5".to_string()]);
    auth_snapshot.api_key_allowed_models = Some(vec!["gpt-5".to_string()]);

    let selection = enumerate_minimal_candidate_selection_with_required_capabilities(
        &state,
        "openai:chat",
        "gpt-5.2",
        false,
        Some(&auth_snapshot),
        None,
        false,
    )
    .await
    .expect("selection should succeed");

    assert_eq!(selection.len(), 1);
    assert_eq!(selection[0].global_model_name, "gpt-5");
}

#[tokio::test]
async fn enumerate_minimal_candidate_selection_gates_model_directive_fallback() {
    let mut row = sample_row();
    row.global_model_name = "gpt-5.4".to_string();
    row.model_provider_model_name = "gpt-5.4-upstream".to_string();

    let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
        row,
    ]));
    let quotas = Arc::new(InMemoryProviderQuotaRepository::seed(vec![]));
    let state = GatewayDataState::with_candidate_selection_and_quota_for_tests(candidates, quotas);

    let disabled = enumerate_minimal_candidate_selection_with_required_capabilities(
        &state,
        "openai:chat",
        "gpt-5.4-high",
        false,
        None,
        None,
        false,
    )
    .await
    .expect("selection should succeed");
    assert!(disabled.is_empty());

    let enabled = enumerate_minimal_candidate_selection_with_required_capabilities(
        &state,
        "openai:chat",
        "gpt-5.4-high",
        false,
        None,
        None,
        true,
    )
    .await
    .expect("selection should succeed");
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].global_model_name, "gpt-5.4");
}
