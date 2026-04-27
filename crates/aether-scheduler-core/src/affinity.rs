use sha2::{Digest, Sha256};

use crate::SchedulerMinimalCandidateSelectionCandidate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerAffinityTarget {
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
}

pub fn build_scheduler_affinity_cache_key_for_api_key_id(
    api_key_id: &str,
    api_format: &str,
    global_model_name: &str,
) -> Option<String> {
    let api_key_id = api_key_id.trim();
    if api_key_id.is_empty() {
        return None;
    }
    let api_format = crate::normalize_api_format(api_format);
    let global_model_name = global_model_name.trim();
    if api_format.is_empty() || global_model_name.is_empty() {
        return None;
    }

    Some(format!(
        "scheduler_affinity:{api_key_id}:{api_format}:{global_model_name}"
    ))
}

pub fn candidate_affinity_hash(
    affinity_key: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(affinity_key.as_bytes());
    hasher.update(b":");
    hasher.update(candidate.provider_id.as_bytes());
    hasher.update(b":");
    hasher.update(candidate.endpoint_id.as_bytes());
    hasher.update(b":");
    hasher.update(candidate.key_id.as_bytes());
    let digest = hasher.finalize();
    u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

pub fn matches_affinity_target(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    target: &SchedulerAffinityTarget,
) -> bool {
    candidate.provider_id == target.provider_id
        && candidate.endpoint_id == target.endpoint_id
        && candidate.key_id == target.key_id
}

pub fn candidate_key(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> (String, String, String) {
    (
        candidate.provider_id.clone(),
        candidate.endpoint_id.clone(),
        candidate.key_id.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        build_scheduler_affinity_cache_key_for_api_key_id, candidate_affinity_hash, candidate_key,
        matches_affinity_target, SchedulerAffinityTarget,
    };
    use crate::SchedulerMinimalCandidateSelectionCandidate;

    fn sample_candidate(id: &str) -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: format!("provider-{id}"),
            provider_name: format!("Provider {id}"),
            provider_type: "custom".to_string(),
            provider_priority: 1,
            endpoint_id: format!("endpoint-{id}"),
            endpoint_api_format: "openai:chat".to_string(),
            key_id: format!("key-{id}"),
            key_name: format!("Key {id}"),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 1,
            key_global_priority_for_format: Some(1),
            key_capabilities: None,
            model_id: format!("model-{id}"),
            global_model_id: format!("global-model-{id}"),
            global_model_name: "gpt-5".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            mapping_matched_model: None,
        }
    }

    #[test]
    fn builds_normalized_scheduler_affinity_cache_key() {
        assert_eq!(
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "OPENAI:CHAT", "gpt-5"),
            Some("scheduler_affinity:api-key-1:openai:chat:gpt-5".to_string())
        );
    }

    #[test]
    fn rejects_blank_affinity_key_components() {
        assert_eq!(
            build_scheduler_affinity_cache_key_for_api_key_id("", "openai:chat", "gpt-5"),
            None
        );
        assert_eq!(
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "", "gpt-5"),
            None
        );
        assert_eq!(
            build_scheduler_affinity_cache_key_for_api_key_id("api-key-1", "openai:chat", ""),
            None
        );
    }

    #[test]
    fn affinity_hash_is_candidate_specific() {
        let left = sample_candidate("1");
        let right = sample_candidate("2");

        assert_ne!(
            candidate_affinity_hash("api-key-1", &left),
            candidate_affinity_hash("api-key-1", &right)
        );
    }

    #[test]
    fn affinity_target_and_candidate_key_reuse_candidate_identity() {
        let candidate = sample_candidate("1");
        let target = SchedulerAffinityTarget {
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
        };

        assert!(matches_affinity_target(&candidate, &target));
        assert_eq!(
            candidate_key(&candidate),
            (
                "provider-1".to_string(),
                "endpoint-1".to_string(),
                "key-1".to_string()
            )
        );
    }
}
