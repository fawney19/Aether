use std::collections::BTreeSet;

use aether_data_contracts::repository::candidates::build_decision_trace;
use aether_data_contracts::DataLayerError;

use super::state::GatewayDataState;

pub(crate) use aether_data_contracts::repository::candidates::{
    DecisionTrace, DecisionTraceCandidate,
};

pub(crate) async fn read_decision_trace(
    state: &GatewayDataState,
    request_id: &str,
    attempted_only: bool,
) -> Result<Option<DecisionTrace>, DataLayerError> {
    let Some(trace) = state
        .read_request_candidate_trace(request_id, attempted_only)
        .await?
    else {
        return Ok(None);
    };

    let provider_ids = unique_ids(
        trace
            .candidates
            .iter()
            .filter_map(|item| item.provider_id.as_ref()),
    );
    let endpoint_ids = unique_ids(
        trace
            .candidates
            .iter()
            .filter_map(|item| item.endpoint_id.as_ref()),
    );
    let key_ids = unique_ids(
        trace
            .candidates
            .iter()
            .filter_map(|item| item.key_id.as_ref()),
    );

    let providers = state
        .list_provider_catalog_providers_by_ids(&provider_ids)
        .await?;
    let endpoints = state
        .list_provider_catalog_endpoints_by_ids(&endpoint_ids)
        .await?;
    let keys = state.list_provider_catalog_keys_by_ids(&key_ids).await?;

    Ok(Some(build_decision_trace(
        trace, providers, endpoints, keys,
    )))
}

fn unique_ids<'a>(items: impl Iterator<Item = &'a String>) -> Vec<String> {
    items
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    use super::{read_decision_trace, DecisionTrace, DecisionTraceCandidate};
    use crate::data::candidates::RequestCandidateFinalStatus;
    use crate::data::GatewayDataState;

    fn sample_candidate(request_id: &str) -> StoredRequestCandidate {
        StoredRequestCandidate::new(
            "cand-1".to_string(),
            request_id.to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            RequestCandidateStatus::Failed,
            None,
            false,
            Some(502),
            Some("bad_gateway".to_string()),
            Some("upstream failed".to_string()),
            Some(37),
            Some(1),
            None,
            Some(serde_json::json!({"cache_1h": true})),
            100_000,
            Some(101_000),
            Some(102_000),
        )
        .expect("candidate should build")
    }

    fn sample_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "OpenAI".to_string(),
            Some("https://openai.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
    }

    fn sample_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
    }

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "provider-key-1".to_string(),
            "provider-1".to_string(),
            "prod-key".to_string(),
            "api_key".to_string(),
            Some(serde_json::json!({"cache_1h": true})),
            true,
        )
        .expect("key should build")
    }

    #[tokio::test]
    async fn enriches_request_candidate_trace_with_provider_catalog_metadata() {
        let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
            sample_candidate("req-1"),
        ]));
        let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![sample_key()],
        ));
        let state = GatewayDataState::with_decision_trace_readers_for_tests(
            request_candidates,
            provider_catalog,
        );

        let trace = read_decision_trace(&state, "req-1", true)
            .await
            .expect("trace should read")
            .expect("trace should exist");

        assert_eq!(
            trace,
            DecisionTrace {
                request_id: "req-1".to_string(),
                total_candidates: 1,
                final_status: RequestCandidateFinalStatus::Failed,
                total_latency_ms: 37,
                candidates: vec![DecisionTraceCandidate {
                    candidate: sample_candidate("req-1"),
                    provider_name: Some("OpenAI".to_string()),
                    provider_website: Some("https://openai.com".to_string()),
                    provider_type: Some("custom".to_string()),
                    provider_priority: Some(0),
                    provider_keep_priority_on_conversion: Some(false),
                    provider_enable_format_conversion: Some(false),
                    endpoint_api_format: Some("openai:chat".to_string()),
                    endpoint_api_family: Some("openai".to_string()),
                    endpoint_kind: Some("chat".to_string()),
                    endpoint_format_acceptance_config: None,
                    provider_key_name: Some("prod-key".to_string()),
                    provider_key_auth_type: Some("api_key".to_string()),
                    provider_key_api_formats: None,
                    provider_key_internal_priority: Some(50),
                    provider_key_global_priority_by_format: None,
                    provider_key_capabilities: Some(serde_json::json!({"cache_1h": true})),
                    provider_key_is_active: Some(true),
                }],
            }
        );
    }
}
