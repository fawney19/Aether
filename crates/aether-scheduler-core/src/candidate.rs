use std::collections::{BTreeMap, BTreeSet};

use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use aether_data_contracts::repository::candidates::StoredRequestCandidate;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_data_contracts::DataLayerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SchedulerPriorityMode {
    #[default]
    Provider,
    GlobalKey,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SchedulerMinimalCandidateSelectionCandidate {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: String,
    pub provider_priority: i32,
    pub endpoint_id: String,
    pub endpoint_api_format: String,
    pub key_id: String,
    pub key_name: String,
    pub key_auth_type: String,
    pub key_internal_priority: i32,
    pub key_global_priority_for_format: Option<i32>,
    pub key_capabilities: Option<serde_json::Value>,
    pub model_id: String,
    pub global_model_id: String,
    pub global_model_name: String,
    pub selected_provider_model_name: String,
    pub mapping_matched_model: Option<String>,
}

pub struct BuildMinimalCandidateSelectionInput<'a> {
    pub rows: Vec<StoredMinimalCandidateSelectionRow>,
    pub normalized_api_format: &'a str,
    pub requested_model_name: &'a str,
    pub resolved_global_model_name: &'a str,
    pub require_streaming: bool,
    pub required_capabilities: Option<&'a serde_json::Value>,
    pub auth_constraints: Option<&'a crate::SchedulerAuthConstraints>,
    pub affinity_key: Option<&'a str>,
    pub priority_mode: SchedulerPriorityMode,
}

pub fn candidate_supports_required_capability(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    required_capability: &str,
) -> bool {
    let required_capability = required_capability.trim();
    if required_capability.is_empty() {
        return true;
    }
    let Some(capabilities) = candidate.key_capabilities.as_ref() else {
        return false;
    };

    if let Some(object) = capabilities.as_object() {
        return object.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case(required_capability)
                && match value {
                    serde_json::Value::Bool(value) => *value,
                    serde_json::Value::String(value) => value.eq_ignore_ascii_case("true"),
                    serde_json::Value::Number(value) => {
                        value.as_i64().is_some_and(|value| value > 0)
                    }
                    _ => false,
                }
        });
    }

    if let Some(items) = capabilities.as_array() {
        return items.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(required_capability))
        });
    }

    false
}

pub fn requested_capability_priority_for_candidate(
    required_capabilities: Option<&serde_json::Value>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> (u32, u32) {
    let Some(required_capabilities) = required_capabilities.and_then(serde_json::Value::as_object)
    else {
        return (0, 0);
    };

    let mut exclusive_misses = 0u32;
    let mut compatible_misses = 0u32;
    for (capability, value) in required_capabilities {
        if !requested_capability_is_enabled(value) {
            continue;
        }
        if candidate_supports_required_capability(candidate, capability) {
            continue;
        }
        if requested_capability_is_compatible(capability) {
            compatible_misses += 1;
        } else {
            exclusive_misses += 1;
        }
    }

    (exclusive_misses, compatible_misses)
}

pub fn auth_api_key_concurrency_limit_reached(
    recent_candidates: &[StoredRequestCandidate],
    now_unix_secs: u64,
    api_key_id: &str,
    concurrent_limit: usize,
) -> bool {
    if api_key_id.trim().is_empty() || concurrent_limit == 0 {
        return false;
    }

    crate::count_recent_active_requests_for_api_key(recent_candidates, api_key_id, now_unix_secs)
        >= concurrent_limit
}

pub fn build_minimal_candidate_selection(
    input: BuildMinimalCandidateSelectionInput<'_>,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, DataLayerError> {
    let BuildMinimalCandidateSelectionInput {
        rows,
        normalized_api_format,
        requested_model_name,
        resolved_global_model_name,
        require_streaming,
        required_capabilities,
        auth_constraints,
        affinity_key,
        priority_mode,
    } = input;

    if normalized_api_format.is_empty() {
        return Ok(Vec::new());
    }
    if !crate::auth_constraints_allow_api_format(auth_constraints, normalized_api_format) {
        return Ok(Vec::new());
    }
    if !crate::auth_constraints_allow_model(
        auth_constraints,
        requested_model_name,
        resolved_global_model_name,
    ) {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::new();
    for row in rows {
        if !crate::auth_constraints_allow_provider(
            auth_constraints,
            &row.provider_id,
            &row.provider_name,
        ) {
            continue;
        }
        if require_streaming && !row.supports_streaming() {
            continue;
        }
        let Some((selected_provider_model_name, mapping_matched_model)) =
            crate::resolve_provider_model_name(&row, requested_model_name, normalized_api_format)
        else {
            continue;
        };

        candidates.push(SchedulerMinimalCandidateSelectionCandidate {
            provider_id: row.provider_id,
            provider_name: row.provider_name,
            provider_type: row.provider_type,
            provider_priority: row.provider_priority,
            endpoint_id: row.endpoint_id,
            endpoint_api_format: row.endpoint_api_format,
            key_id: row.key_id,
            key_name: row.key_name,
            key_auth_type: row.key_auth_type,
            key_internal_priority: row.key_internal_priority,
            key_global_priority_for_format: crate::extract_global_priority_for_format(
                row.key_global_priority_by_format.as_ref(),
                normalized_api_format,
            )?,
            key_capabilities: row.key_capabilities,
            model_id: row.model_id,
            global_model_id: row.global_model_id,
            global_model_name: row.global_model_name,
            selected_provider_model_name,
            mapping_matched_model,
        });
    }

    candidates.sort_by(|left, right| {
        requested_capability_priority_for_candidate(required_capabilities, left)
            .cmp(&requested_capability_priority_for_candidate(
                required_capabilities,
                right,
            ))
            .then_with(|| {
                compare_candidates_by_priority_mode(left, right, priority_mode, affinity_key)
            })
    });

    Ok(candidates)
}

fn requested_capability_is_enabled(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => value.eq_ignore_ascii_case("true"),
        serde_json::Value::Number(value) => value.as_i64().is_some_and(|value| value > 0),
        _ => false,
    }
}

fn requested_capability_is_compatible(capability: &str) -> bool {
    matches!(
        capability.trim().to_ascii_lowercase().as_str(),
        "cache_1h" | "context_1m"
    )
}

pub fn compare_candidates_by_priority_mode(
    left: &SchedulerMinimalCandidateSelectionCandidate,
    right: &SchedulerMinimalCandidateSelectionCandidate,
    priority_mode: SchedulerPriorityMode,
    affinity_key: Option<&str>,
) -> std::cmp::Ordering {
    match priority_mode {
        SchedulerPriorityMode::Provider => left
            .provider_priority
            .cmp(&right.provider_priority)
            .then(left.key_internal_priority.cmp(&right.key_internal_priority))
            .then_with(|| crate::compare_affinity_order(left, right, affinity_key))
            .then_with(|| compare_candidate_identity(left, right)),
        SchedulerPriorityMode::GlobalKey => left
            .key_global_priority_for_format
            .unwrap_or(i32::MAX)
            .cmp(&right.key_global_priority_for_format.unwrap_or(i32::MAX))
            .then_with(|| crate::compare_affinity_order(left, right, affinity_key))
            .then(left.provider_priority.cmp(&right.provider_priority))
            .then(left.key_internal_priority.cmp(&right.key_internal_priority))
            .then_with(|| compare_candidate_identity(left, right)),
    }
}

pub fn collect_global_model_names_for_required_capability(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    normalized_api_format: &str,
    required_capability: &str,
    require_streaming: bool,
    auth_constraints: Option<&crate::SchedulerAuthConstraints>,
) -> Vec<String> {
    if normalized_api_format.is_empty() || required_capability.trim().is_empty() {
        return Vec::new();
    }
    if !crate::auth_constraints_allow_api_format(auth_constraints, normalized_api_format) {
        return Vec::new();
    }

    let mut model_names = BTreeSet::new();
    for row in rows {
        if !crate::auth_constraints_allow_provider(
            auth_constraints,
            &row.provider_id,
            &row.provider_name,
        ) {
            continue;
        }
        if !crate::row_supports_required_capability(&row, required_capability) {
            continue;
        }
        if require_streaming && !row.supports_streaming() {
            continue;
        }
        if !crate::auth_constraints_allow_model(
            auth_constraints,
            &row.global_model_name,
            &row.global_model_name,
        ) {
            continue;
        }
        model_names.insert(row.global_model_name);
    }

    model_names.into_iter().collect()
}

pub fn collect_selectable_candidates_from_keys(
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    selectable_keys: &BTreeSet<(String, String, String)>,
    cached_affinity_target: Option<&crate::SchedulerAffinityTarget>,
) -> Vec<SchedulerMinimalCandidateSelectionCandidate> {
    let mut selected = Vec::new();
    let mut emitted_keys = BTreeSet::new();

    if let Some(target) = cached_affinity_target {
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| crate::matches_affinity_target(candidate, target))
            .cloned()
        {
            let key = crate::candidate_key(&candidate);
            if selectable_keys.contains(&key) && emitted_keys.insert(key) {
                selected.push(candidate);
            }
        }
    }

    for candidate in candidates {
        let key = crate::candidate_key(&candidate);
        if !selectable_keys.contains(&key) || !emitted_keys.insert(key) {
            continue;
        }
        selected.push(candidate);
    }

    selected
}

pub fn reorder_candidates_by_scheduler_health(
    candidates: &mut [SchedulerMinimalCandidateSelectionCandidate],
    provider_key_rpm_states: &BTreeMap<String, StoredProviderCatalogKey>,
    required_capabilities: Option<&serde_json::Value>,
    affinity_key: Option<&str>,
    priority_mode: SchedulerPriorityMode,
) {
    candidates.sort_by(|left, right| {
        requested_capability_priority_for_candidate(required_capabilities, left)
            .cmp(&requested_capability_priority_for_candidate(
                required_capabilities,
                right,
            ))
            .then_with(|| match priority_mode {
                SchedulerPriorityMode::Provider => left
                    .provider_priority
                    .cmp(&right.provider_priority)
                    .then(left.key_internal_priority.cmp(&right.key_internal_priority))
                    .then_with(|| {
                        compare_provider_key_health_order(left, right, provider_key_rpm_states)
                    })
                    .then_with(|| crate::compare_affinity_order(left, right, affinity_key))
                    .then_with(|| compare_candidate_identity(left, right)),
                SchedulerPriorityMode::GlobalKey => left
                    .key_global_priority_for_format
                    .unwrap_or(i32::MAX)
                    .cmp(&right.key_global_priority_for_format.unwrap_or(i32::MAX))
                    .then_with(|| {
                        compare_provider_key_health_order(left, right, provider_key_rpm_states)
                    })
                    .then_with(|| crate::compare_affinity_order(left, right, affinity_key))
                    .then(left.provider_priority.cmp(&right.provider_priority))
                    .then(left.key_internal_priority.cmp(&right.key_internal_priority))
                    .then_with(|| compare_candidate_identity(left, right)),
            })
    });
}

#[derive(Clone, Copy, Debug)]
pub struct CandidateRuntimeSelectabilityInput<'a> {
    pub candidate: &'a SchedulerMinimalCandidateSelectionCandidate,
    pub recent_candidates: &'a [StoredRequestCandidate],
    pub provider_concurrent_limits: &'a BTreeMap<String, usize>,
    pub provider_key_rpm_states: &'a BTreeMap<String, StoredProviderCatalogKey>,
    pub now_unix_secs: u64,
    pub cached_affinity_target: Option<&'a crate::SchedulerAffinityTarget>,
    pub provider_quota_blocks_requests: bool,
    pub account_quota_exhausted: bool,
    pub rpm_reset_at: Option<u64>,
}

pub fn candidate_is_selectable_with_runtime_state(
    input: CandidateRuntimeSelectabilityInput<'_>,
) -> bool {
    candidate_runtime_skip_reason_with_state(input).is_none()
}

pub fn candidate_runtime_skip_reason_with_state(
    input: CandidateRuntimeSelectabilityInput<'_>,
) -> Option<&'static str> {
    let CandidateRuntimeSelectabilityInput {
        candidate,
        recent_candidates,
        provider_concurrent_limits,
        provider_key_rpm_states,
        now_unix_secs,
        cached_affinity_target,
        provider_quota_blocks_requests,
        account_quota_exhausted,
        rpm_reset_at,
    } = input;

    if provider_quota_blocks_requests {
        return Some("provider_quota_blocked");
    }
    if account_quota_exhausted {
        return Some("account_quota_exhausted");
    }
    if crate::is_candidate_in_recent_failure_cooldown(
        recent_candidates,
        candidate.provider_id.as_str(),
        candidate.endpoint_id.as_str(),
        candidate.key_id.as_str(),
        now_unix_secs,
    ) {
        return Some("recent_failure_cooldown");
    }
    if provider_concurrent_limits
        .get(&candidate.provider_id)
        .is_some_and(|limit| {
            crate::count_recent_active_requests_for_provider(
                recent_candidates,
                candidate.provider_id.as_str(),
                now_unix_secs,
            ) >= *limit
        })
    {
        return Some("provider_concurrency_limit_reached");
    }

    let is_cached_user = cached_affinity_target
        .is_some_and(|target| crate::matches_affinity_target(candidate, target));
    if let Some(provider_key) = provider_key_rpm_states.get(&candidate.key_id) {
        if crate::is_provider_key_circuit_open(provider_key, candidate.endpoint_api_format.as_str())
        {
            return Some("key_circuit_open");
        }
        if crate::provider_key_health_score(provider_key, candidate.endpoint_api_format.as_str())
            .is_some_and(|score| score <= 0.0)
        {
            return Some("key_health_score_zero");
        }
        if !crate::provider_key_rpm_allows_request_since(
            provider_key,
            recent_candidates,
            now_unix_secs,
            is_cached_user,
            rpm_reset_at,
        ) {
            return Some("key_rpm_exhausted");
        }
    }

    None
}

fn compare_provider_key_health_order(
    left: &SchedulerMinimalCandidateSelectionCandidate,
    right: &SchedulerMinimalCandidateSelectionCandidate,
    provider_key_rpm_states: &BTreeMap<String, StoredProviderCatalogKey>,
) -> std::cmp::Ordering {
    let left_bucket = candidate_provider_key_health_bucket(left, provider_key_rpm_states);
    let right_bucket = candidate_provider_key_health_bucket(right, provider_key_rpm_states);
    right_bucket.cmp(&left_bucket).then_with(|| {
        let left_score = candidate_provider_key_health_score(left, provider_key_rpm_states);
        let right_score = candidate_provider_key_health_score(right, provider_key_rpm_states);
        right_score
            .partial_cmp(&left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn candidate_provider_key_health_bucket(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    provider_key_rpm_states: &BTreeMap<String, StoredProviderCatalogKey>,
) -> Option<crate::ProviderKeyHealthBucket> {
    provider_key_rpm_states
        .get(&candidate.key_id)
        .and_then(|key| {
            crate::provider_key_health_bucket(key, candidate.endpoint_api_format.as_str())
        })
}

fn compare_candidate_identity(
    left: &SchedulerMinimalCandidateSelectionCandidate,
    right: &SchedulerMinimalCandidateSelectionCandidate,
) -> std::cmp::Ordering {
    left.provider_id
        .cmp(&right.provider_id)
        .then(left.endpoint_id.cmp(&right.endpoint_id))
        .then(left.key_id.cmp(&right.key_id))
        .then(
            left.selected_provider_model_name
                .cmp(&right.selected_provider_model_name),
        )
}

fn candidate_provider_key_health_score(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    provider_key_rpm_states: &BTreeMap<String, StoredProviderCatalogKey>,
) -> f64 {
    provider_key_rpm_states
        .get(&candidate.key_id)
        .and_then(|key| {
            crate::effective_provider_key_health_score(key, candidate.endpoint_api_format.as_str())
        })
        .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use aether_data_contracts::repository::candidate_selection::{
        StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
    };
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

    use super::{
        auth_api_key_concurrency_limit_reached, build_minimal_candidate_selection,
        candidate_is_selectable_with_runtime_state, candidate_supports_required_capability,
        collect_global_model_names_for_required_capability,
        collect_selectable_candidates_from_keys, reorder_candidates_by_scheduler_health,
        BuildMinimalCandidateSelectionInput, CandidateRuntimeSelectabilityInput,
        SchedulerMinimalCandidateSelectionCandidate, SchedulerPriorityMode,
    };
    use crate::SchedulerAuthConstraints;

    fn sample_row(id: &str) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: format!("provider-{id}"),
            provider_name: format!("Provider {id}"),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: format!("endpoint-{id}"),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: format!("key-{id}"),
            key_name: format!("prod-{id}"),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: Some(serde_json::json!({"cache_1h": true})),
            key_internal_priority: 50,
            key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 2})),
            model_id: format!("model-{id}"),
            global_model_id: format!("global-model-{id}"),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: Some(vec!["gpt-5(?:\\.\\d+)?".to_string()]),
            global_model_supports_streaming: Some(true),
            model_provider_model_name: format!("gpt-5-upstream-{id}"),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: format!("gpt-5-canary-{id}"),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
            }]),
            model_supports_streaming: None,
            model_is_active: true,
            model_is_available: true,
        }
    }
    fn sample_candidate(
        id: &str,
        capabilities: Option<serde_json::Value>,
    ) -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: format!("provider-{id}"),
            provider_name: format!("Provider {id}"),
            provider_type: "openai".to_string(),
            provider_priority: 0,
            endpoint_id: format!("endpoint-{id}"),
            endpoint_api_format: "openai:chat".to_string(),
            key_id: format!("key-{id}"),
            key_name: format!("key-{id}"),
            key_auth_type: "bearer".to_string(),
            key_internal_priority: 0,
            key_global_priority_for_format: None,
            key_capabilities: capabilities,
            model_id: format!("model-{id}"),
            global_model_id: format!("global-model-{id}"),
            global_model_name: "gpt-5".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            mapping_matched_model: None,
        }
    }

    fn sample_key(id: &str, health_score: f64) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            format!("key-{id}"),
            format!("provider-{id}"),
            format!("key-{id}"),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("provider key should build");
        key.health_by_format = Some(serde_json::json!({
            "openai:chat": {
                "health_score": health_score
            }
        }));
        key
    }

    fn stored_candidate(
        id: &str,
        status: RequestCandidateStatus,
        created_at_unix_ms: i64,
    ) -> StoredRequestCandidate {
        let finished_at_unix_ms = match status {
            RequestCandidateStatus::Pending | RequestCandidateStatus::Streaming => None,
            _ => Some(created_at_unix_ms),
        };
        StoredRequestCandidate::new(
            id.to_string(),
            format!("req-{id}"),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
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
            created_at_unix_ms,
            Some(created_at_unix_ms),
            finished_at_unix_ms,
        )
        .expect("candidate should build")
    }

    #[test]
    fn reads_required_capability_from_object_and_array_forms() {
        assert!(candidate_supports_required_capability(
            &sample_candidate("1", Some(serde_json::json!({"vision": true}))),
            "vision"
        ));
        assert!(candidate_supports_required_capability(
            &sample_candidate("1", Some(serde_json::json!(["vision", "tools"]))),
            "tools"
        ));
        assert!(!candidate_supports_required_capability(
            &sample_candidate("1", Some(serde_json::json!({"vision": false}))),
            "vision"
        ));
    }

    #[test]
    fn builds_minimal_candidate_selection_with_auth_constraints() {
        let mut disallowed = sample_row("2");
        disallowed.provider_id = "provider-blocked".to_string();
        disallowed.provider_name = "Blocked".to_string();

        let constraints = SchedulerAuthConstraints {
            allowed_providers: Some(vec!["provider-1".to_string()]),
            allowed_api_formats: Some(vec!["OPENAI:CHAT".to_string()]),
            allowed_models: Some(vec!["gpt-5".to_string()]),
        };
        let candidates = build_minimal_candidate_selection(BuildMinimalCandidateSelectionInput {
            rows: vec![sample_row("1"), disallowed],
            normalized_api_format: "openai:chat",
            requested_model_name: "gpt-5",
            resolved_global_model_name: "gpt-5",
            require_streaming: false,
            required_capabilities: None,
            auth_constraints: Some(&constraints),
            affinity_key: None,
            priority_mode: SchedulerPriorityMode::Provider,
        })
        .expect("candidate selection should build");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provider_id, "provider-1");
        assert_eq!(candidates[0].selected_provider_model_name, "gpt-5-canary-1");
    }

    #[test]
    fn collects_global_model_names_for_required_capability_with_auth_constraints() {
        let mut disallowed = sample_row("2");
        disallowed.global_model_name = "gpt-4.1".to_string();
        disallowed.provider_id = "provider-blocked".to_string();
        disallowed.provider_name = "Blocked".to_string();

        let constraints = SchedulerAuthConstraints {
            allowed_providers: Some(vec!["provider-1".to_string()]),
            allowed_api_formats: Some(vec!["openai:chat".to_string()]),
            allowed_models: Some(vec!["gpt-5".to_string()]),
        };
        let model_names = collect_global_model_names_for_required_capability(
            vec![sample_row("1"), disallowed],
            "openai:chat",
            "cache_1h",
            false,
            Some(&constraints),
        );

        assert_eq!(model_names, vec!["gpt-5".to_string()]);
    }

    #[test]
    fn minimal_candidate_selection_prefers_matching_requested_capabilities_before_priority() {
        let mut missing_capability = sample_row("1");
        missing_capability.key_capabilities = Some(serde_json::json!({"cache_1h": false}));
        missing_capability.provider_priority = 0;

        let mut matching_capability = sample_row("2");
        matching_capability.key_capabilities = Some(serde_json::json!({"cache_1h": true}));
        matching_capability.provider_priority = 10;

        let required_capabilities = serde_json::json!({"cache_1h": true});
        let candidates = build_minimal_candidate_selection(BuildMinimalCandidateSelectionInput {
            rows: vec![missing_capability, matching_capability],
            normalized_api_format: "openai:chat",
            requested_model_name: "gpt-5",
            resolved_global_model_name: "gpt-5",
            require_streaming: false,
            required_capabilities: Some(&required_capabilities),
            auth_constraints: None,
            affinity_key: None,
            priority_mode: SchedulerPriorityMode::Provider,
        })
        .expect("candidate selection should build");

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].key_id, "key-2");
        assert_eq!(candidates[1].key_id, "key-1");
    }

    #[test]
    fn reorders_candidates_by_health_before_affinity_tiebreak() {
        let mut candidates = vec![
            sample_candidate("1", None),
            sample_candidate("2", None),
            sample_candidate("3", None),
        ];
        let provider_key_rpm_states = BTreeMap::from([
            ("key-1".to_string(), sample_key("1", 0.95)),
            ("key-2".to_string(), sample_key("2", 0.40)),
            ("key-3".to_string(), sample_key("3", 0.95)),
        ]);

        reorder_candidates_by_scheduler_health(
            &mut candidates,
            &provider_key_rpm_states,
            None,
            Some("api-key-1"),
            SchedulerPriorityMode::GlobalKey,
        );

        assert_ne!(candidates[0].key_id, "key-2");
        assert_ne!(candidates[1].key_id, "key-2");
        assert_eq!(candidates[2].key_id, "key-2");
    }

    #[test]
    fn collects_selectable_candidates_with_affinity_priority_and_dedup() {
        let candidates = vec![
            sample_candidate("1", None),
            sample_candidate("2", None),
            sample_candidate("1", None),
        ];
        let selectable_keys = BTreeSet::from([
            (
                "provider-1".to_string(),
                "endpoint-1".to_string(),
                "key-1".to_string(),
            ),
            (
                "provider-2".to_string(),
                "endpoint-2".to_string(),
                "key-2".to_string(),
            ),
        ]);
        let selected = collect_selectable_candidates_from_keys(
            candidates,
            &selectable_keys,
            Some(&crate::SchedulerAffinityTarget {
                provider_id: "provider-2".to_string(),
                endpoint_id: "endpoint-2".to_string(),
                key_id: "key-2".to_string(),
            }),
        );

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].key_id, "key-2");
        assert_eq!(selected[1].key_id, "key-1");
    }

    #[test]
    fn candidate_selectability_respects_provider_concurrency_limit() {
        let recent_candidates = vec![stored_candidate("one", RequestCandidateStatus::Pending, 95)];
        let provider_concurrent_limits = BTreeMap::from([("provider-1".to_string(), 1)]);

        assert!(!candidate_is_selectable_with_runtime_state(
            CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &recent_candidates,
                provider_concurrent_limits: &provider_concurrent_limits,
                provider_key_rpm_states: &BTreeMap::new(),
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                rpm_reset_at: None,
            },
        ));
    }

    #[test]
    fn candidate_selectability_rejects_quota_or_zero_health() {
        let provider_key_rpm_states = BTreeMap::from([("key-1".to_string(), sample_key("1", 0.0))]);

        assert!(!candidate_is_selectable_with_runtime_state(
            CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &[],
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &provider_key_rpm_states,
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                rpm_reset_at: None,
            },
        ));
        assert!(!candidate_is_selectable_with_runtime_state(
            CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &[],
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &BTreeMap::new(),
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: true,
                account_quota_exhausted: false,
                rpm_reset_at: None,
            },
        ));
    }

    #[test]
    fn candidate_selectability_rejects_exhausted_account_quota() {
        assert!(!candidate_is_selectable_with_runtime_state(
            CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &[],
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &BTreeMap::new(),
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: true,
                rpm_reset_at: None,
            },
        ));
    }

    #[test]
    fn detects_auth_api_key_concurrency_limit_from_recent_active_requests() {
        let recent_candidates = vec![StoredRequestCandidate::new(
            "one".to_string(),
            "req-one".to_string(),
            None,
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
            95,
            Some(95),
            None,
        )
        .expect("candidate should build")];

        assert!(auth_api_key_concurrency_limit_reached(
            &recent_candidates,
            100,
            "api-key-1",
            1,
        ));
        assert!(!auth_api_key_concurrency_limit_reached(
            &recent_candidates,
            100,
            "api-key-1",
            2,
        ));
    }
}
