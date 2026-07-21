use super::{resolved_oauth_invalid_state, RefreshOutcome};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

fn sample_key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "key-glm".to_string(),
        "provider-glm".to_string(),
        "GLM key".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
}

fn candidate_invalid_outcome(parsed_probe_count: usize) -> RefreshOutcome {
    RefreshOutcome {
        metadata: serde_json::Map::new(),
        parsed_probe_count,
        oauth_invalid_at_unix_secs: Some(1_234),
        oauth_invalid_reason: Some("token expired or incorrect".to_string()),
    }
}

#[test]
fn mixed_success_does_not_persist_candidate_oauth_invalid() {
    // Given
    let key = sample_key();
    let outcome = candidate_invalid_outcome(1);

    // When
    let state = resolved_oauth_invalid_state(&key, &outcome);

    // Then
    assert_eq!(state, (None, None));
}

#[test]
fn all_auth_failures_persist_candidate_oauth_invalid() {
    // Given
    let key = sample_key();
    let outcome = candidate_invalid_outcome(0);

    // When
    let state = resolved_oauth_invalid_state(&key, &outcome);

    // Then
    assert_eq!(
        state,
        (Some(1_234), Some("token expired or incorrect".to_string()))
    );
}

#[test]
fn successful_probe_preserves_existing_account_block() {
    // Given
    let mut key = sample_key();
    key.oauth_invalid_at_unix_secs = Some(999);
    key.oauth_invalid_reason = Some("[ACCOUNT_BLOCK] account deactivated".to_string());
    let outcome = candidate_invalid_outcome(1);

    // When
    let state = resolved_oauth_invalid_state(&key, &outcome);

    // Then
    assert_eq!(
        state,
        (
            Some(999),
            Some("[ACCOUNT_BLOCK] account deactivated".to_string())
        )
    );
}
