pub mod capability;
pub mod enumeration;
pub mod selectability;
pub mod types;

pub use capability::{
    candidate_supports_required_capability, requested_capability_priority_for_candidate,
};
pub use enumeration::{
    collect_global_model_names_for_required_capability, enumerate_minimal_candidate_selection,
};
pub use selectability::{
    auth_api_key_concurrency_limit_reached, candidate_is_selectable_with_runtime_state,
    candidate_runtime_skip_reason_with_state, CandidateRuntimeSelectabilityInput,
};
pub use types::{
    EnumerateMinimalCandidateSelectionInput, SchedulerMinimalCandidateSelectionCandidate,
    SchedulerPriorityMode,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_data_contracts::repository::candidate_selection::{
        StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
    };
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

    use super::{
        auth_api_key_concurrency_limit_reached, candidate_is_selectable_with_runtime_state,
        candidate_runtime_skip_reason_with_state, candidate_supports_required_capability,
        collect_global_model_names_for_required_capability, CandidateRuntimeSelectabilityInput,
        EnumerateMinimalCandidateSelectionInput, SchedulerMinimalCandidateSelectionCandidate,
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

    fn sample_key_with_concurrent_limit(
        id: &str,
        concurrent_limit: Option<i32>,
    ) -> StoredProviderCatalogKey {
        let mut key = sample_key(id, 1.0);
        key.concurrent_limit = concurrent_limit;
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
    fn enumerates_minimal_candidate_selection_with_auth_constraints() {
        let mut disallowed = sample_row("2");
        disallowed.provider_id = "provider-blocked".to_string();
        disallowed.provider_name = "Blocked".to_string();

        let constraints = SchedulerAuthConstraints {
            allowed_providers: Some(vec!["provider-1".to_string()]),
            allowed_api_formats: Some(vec!["OPENAI:CHAT".to_string()]),
            allowed_models: Some(vec!["gpt-5".to_string()]),
        };
        let candidates =
            super::enumerate_minimal_candidate_selection(EnumerateMinimalCandidateSelectionInput {
                rows: vec![sample_row("1"), disallowed],
                normalized_api_format: "openai:chat",
                requested_model_name: "gpt-5",
                resolved_global_model_name: "gpt-5",
                require_streaming: false,
                required_capabilities: None,
                auth_constraints: Some(&constraints),
            })
            .expect("candidate selection should build");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provider_id, "provider-1");
        assert_eq!(candidates[0].selected_provider_model_name, "gpt-5-canary-1");
    }

    #[test]
    fn enumeration_preserves_theoretical_candidate_order_without_final_sorting() {
        let mut later_priority = sample_row("1");
        later_priority.provider_priority = 10;
        let mut earlier_priority = sample_row("2");
        earlier_priority.provider_priority = 0;

        let candidates =
            super::enumerate_minimal_candidate_selection(EnumerateMinimalCandidateSelectionInput {
                rows: vec![later_priority, earlier_priority],
                normalized_api_format: "openai:chat",
                requested_model_name: "gpt-5",
                resolved_global_model_name: "gpt-5",
                require_streaming: false,
                required_capabilities: None,
                auth_constraints: None,
            })
            .expect("candidate enumeration should build");

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].provider_id, "provider-1");
        assert_eq!(candidates[1].provider_id, "provider-2");
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
    fn requested_capability_priority_counts_missing_compatible_capabilities() {
        let mut missing_capability = sample_row("1");
        missing_capability.key_capabilities = Some(serde_json::json!({"cache_1h": false}));
        missing_capability.provider_priority = 0;

        let mut matching_capability = sample_row("2");
        matching_capability.key_capabilities = Some(serde_json::json!({"cache_1h": true}));
        matching_capability.provider_priority = 10;

        let required_capabilities = serde_json::json!({"cache_1h": true});
        let candidates =
            super::enumerate_minimal_candidate_selection(EnumerateMinimalCandidateSelectionInput {
                rows: vec![missing_capability, matching_capability],
                normalized_api_format: "openai:chat",
                requested_model_name: "gpt-5",
                resolved_global_model_name: "gpt-5",
                require_streaming: false,
                required_capabilities: Some(&required_capabilities),
                auth_constraints: None,
            })
            .expect("candidate selection should build");
        let priority = candidates
            .iter()
            .map(|candidate| {
                super::requested_capability_priority_for_candidate(
                    Some(&required_capabilities),
                    candidate,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(candidates.len(), 2);
        assert_eq!(priority, vec![(0, 1), (0, 0)]);
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
                oauth_invalid: false,
                rpm_reset_at: None,
            },
        ));
    }

    #[test]
    fn provider_key_concurrency_limit_unset_or_zero_is_unlimited() {
        let recent_candidates = vec![stored_candidate("one", RequestCandidateStatus::Pending, 95)];
        for concurrent_limit in [None, Some(0)] {
            let provider_key_rpm_states = BTreeMap::from([(
                "key-1".to_string(),
                sample_key_with_concurrent_limit("1", concurrent_limit),
            )]);

            assert_eq!(
                candidate_runtime_skip_reason_with_state(CandidateRuntimeSelectabilityInput {
                    candidate: &sample_candidate("1", None),
                    recent_candidates: &recent_candidates,
                    provider_concurrent_limits: &BTreeMap::new(),
                    provider_key_rpm_states: &provider_key_rpm_states,
                    now_unix_secs: 100,
                    cached_affinity_target: None,
                    provider_quota_blocks_requests: false,
                    account_quota_exhausted: false,
                    oauth_invalid: false,
                    rpm_reset_at: None,
                }),
                None
            );
        }
    }

    #[test]
    fn provider_key_concurrency_limit_rejects_pending_active_with_exact_skip_reason() {
        let recent_candidates = vec![stored_candidate("one", RequestCandidateStatus::Pending, 95)];
        let provider_key_rpm_states = BTreeMap::from([(
            "key-1".to_string(),
            sample_key_with_concurrent_limit("1", Some(1)),
        )]);

        assert_eq!(
            candidate_runtime_skip_reason_with_state(CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &recent_candidates,
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &provider_key_rpm_states,
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                oauth_invalid: false,
                rpm_reset_at: None,
            }),
            Some("provider_key_concurrency_limit_reached")
        );
    }

    #[test]
    fn provider_key_concurrency_limit_rejects_streaming_active() {
        let recent_candidates = vec![stored_candidate(
            "streaming",
            RequestCandidateStatus::Streaming,
            95,
        )];
        let provider_key_rpm_states = BTreeMap::from([(
            "key-1".to_string(),
            sample_key_with_concurrent_limit("1", Some(1)),
        )]);

        assert_eq!(
            candidate_runtime_skip_reason_with_state(CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &recent_candidates,
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &provider_key_rpm_states,
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                oauth_invalid: false,
                rpm_reset_at: None,
            }),
            Some("provider_key_concurrency_limit_reached")
        );
    }

    #[test]
    fn provider_key_concurrency_limit_ignores_finished_and_stale_active_requests() {
        let recent_candidates = vec![
            stored_candidate("finished", RequestCandidateStatus::Success, 95),
            stored_candidate("failed", RequestCandidateStatus::Failed, 96),
            stored_candidate("cancelled", RequestCandidateStatus::Cancelled, 97),
            stored_candidate("stale", RequestCandidateStatus::Pending, 699_000),
        ];
        let provider_key_rpm_states = BTreeMap::from([(
            "key-1".to_string(),
            sample_key_with_concurrent_limit("1", Some(1)),
        )]);

        assert_eq!(
            candidate_runtime_skip_reason_with_state(CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &recent_candidates,
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &provider_key_rpm_states,
                now_unix_secs: 1_000,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                oauth_invalid: false,
                rpm_reset_at: None,
            }),
            None
        );
    }

    #[test]
    fn provider_key_concurrency_limit_missing_state_does_not_skip() {
        let recent_candidates = vec![stored_candidate("one", RequestCandidateStatus::Pending, 95)];

        assert_eq!(
            candidate_runtime_skip_reason_with_state(CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &recent_candidates,
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &BTreeMap::new(),
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                oauth_invalid: false,
                rpm_reset_at: None,
            }),
            None
        );
    }

    #[test]
    fn provider_key_concurrency_limit_preserves_key_circuit_and_rpm_checks() {
        let mut circuit_open_key = sample_key_with_concurrent_limit("1", Some(2));
        circuit_open_key.circuit_breaker_by_format = Some(serde_json::json!({
            "openai:chat": {"open": true}
        }));
        let provider_key_rpm_states = BTreeMap::from([("key-1".to_string(), circuit_open_key)]);
        assert_eq!(
            candidate_runtime_skip_reason_with_state(CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &[],
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &provider_key_rpm_states,
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                oauth_invalid: false,
                rpm_reset_at: None,
            }),
            Some("key_circuit_open")
        );

        let recent_candidates = vec![stored_candidate(
            "one",
            RequestCandidateStatus::Pending,
            95_000,
        )];
        let provider_key_rpm_states = BTreeMap::from([(
            "key-1".to_string(),
            sample_key_with_concurrent_limit("1", Some(2)).with_rate_limit_fields(
                Some(1),
                Some(2),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
        )]);

        assert_eq!(
            candidate_runtime_skip_reason_with_state(CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &recent_candidates,
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &provider_key_rpm_states,
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                oauth_invalid: false,
                rpm_reset_at: None,
            }),
            Some("key_rpm_exhausted")
        );
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
                oauth_invalid: false,
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
                oauth_invalid: false,
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
                oauth_invalid: false,
                rpm_reset_at: None,
            },
        ));
    }

    #[test]
    fn candidate_selectability_rejects_oauth_invalid_keys() {
        assert!(!candidate_is_selectable_with_runtime_state(
            CandidateRuntimeSelectabilityInput {
                candidate: &sample_candidate("1", None),
                recent_candidates: &[],
                provider_concurrent_limits: &BTreeMap::new(),
                provider_key_rpm_states: &BTreeMap::new(),
                now_unix_secs: 100,
                cached_affinity_target: None,
                provider_quota_blocks_requests: false,
                account_quota_exhausted: false,
                oauth_invalid: true,
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
