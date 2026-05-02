use aether_scheduler_core::parse_request_candidate_report_context;
use serde_json::Value;

use crate::provider_transport::GatewayProviderTransportSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionAttemptIdentity {
    pub(crate) candidate_index: u32,
    pub(crate) retry_index: u32,
    pub(crate) pool_key_index: Option<u32>,
}

impl ExecutionAttemptIdentity {
    pub(crate) const fn new(candidate_index: u32, retry_index: u32) -> Self {
        Self {
            candidate_index,
            retry_index,
            pool_key_index: None,
        }
    }

    pub(crate) const fn with_pool_key_index(mut self, pool_key_index: Option<u32>) -> Self {
        self.pool_key_index = pool_key_index;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LocalExecutionCandidateMetadata {
    pub(crate) candidate_group_id: Option<String>,
    pub(crate) pool_key_index: Option<u32>,
}

pub(crate) fn attempt_identity_from_report_context(
    report_context: Option<&Value>,
) -> Option<ExecutionAttemptIdentity> {
    let metadata = parse_request_candidate_report_context(report_context)?;
    let candidate_metadata = local_execution_candidate_metadata_from_report_context(report_context);

    Some(ExecutionAttemptIdentity {
        candidate_index: metadata.candidate_index?,
        retry_index: metadata.retry_index,
        pool_key_index: candidate_metadata.pool_key_index,
    })
}

pub(crate) fn local_execution_candidate_metadata_from_report_context(
    report_context: Option<&Value>,
) -> LocalExecutionCandidateMetadata {
    LocalExecutionCandidateMetadata {
        candidate_group_id: report_context
            .and_then(Value::as_object)
            .and_then(|value| value.get("candidate_group_id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        pool_key_index: report_context
            .and_then(|value| value.get("pool_key_index"))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
    }
}

pub(crate) fn build_local_attempt_identities(
    candidate_index: u32,
    transport: &GatewayProviderTransportSnapshot,
) -> Vec<ExecutionAttemptIdentity> {
    let attempt_slots = local_attempt_slot_count(transport);
    (0..attempt_slots)
        .map(|retry_index| ExecutionAttemptIdentity::new(candidate_index, retry_index))
        .collect()
}

pub(crate) fn local_attempt_slot_count(transport: &GatewayProviderTransportSnapshot) -> u32 {
    local_attempt_slots_from_transport(transport).unwrap_or(1)
}

/// For endpoint/provider table fields, `2` is the legacy admin default and is
/// treated as "not explicitly configured" so existing local-execution behaviour
/// (one attempt slot per candidate) stays unchanged. Values `0`, `1`, and `>2`
/// are treated as explicit.
const LEGACY_DEFAULT_MAX_RETRIES: u32 = 2;

/// Upper bound on local attempt slots. This is intentionally stricter than
/// admin max_retries validation to prevent unbounded pre-materialization from
/// arbitrarily large JSON config values.
const MAX_LOCAL_ATTEMPT_SLOTS: u32 = 99;

fn local_attempt_slots_from_transport(transport: &GatewayProviderTransportSnapshot) -> Option<u32> {
    let rules = transport
        .provider
        .config
        .as_ref()
        .and_then(|config| config.get("failover_rules"))
        .and_then(Value::as_object);

    rules
        .and_then(|value| value.get("max_retries"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| {
            transport
                .endpoint
                .max_retries
                .and_then(|value| u32::try_from(value).ok())
                .filter(|&value| value != LEGACY_DEFAULT_MAX_RETRIES)
        })
        .or_else(|| {
            transport
                .provider
                .max_retries
                .and_then(|value| u32::try_from(value).ok())
                .filter(|&value| value != LEGACY_DEFAULT_MAX_RETRIES)
        })
        .map(|value| value.clamp(1, MAX_LOCAL_ATTEMPT_SLOTS))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        attempt_identity_from_report_context, build_local_attempt_identities,
        local_execution_candidate_metadata_from_report_context, ExecutionAttemptIdentity,
        LocalExecutionCandidateMetadata,
    };
    use crate::provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(
        provider_max_retries: Option<i32>,
        endpoint_max_retries: Option<i32>,
        provider_config: Option<serde_json::Value>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "OpenAI".to_string(),
                provider_type: "llm".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: provider_max_retries,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: provider_config,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: endpoint_max_retries,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "primary".to_string(),
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn build_local_attempt_identities_defaults_to_single_attempt() {
        let identities = build_local_attempt_identities(3, &sample_transport(None, None, None));

        assert_eq!(identities, vec![ExecutionAttemptIdentity::new(3, 0)]);
    }

    #[test]
    fn build_local_attempt_identities_prefer_failover_rules_over_endpoint_and_provider() {
        let identities = build_local_attempt_identities(
            1,
            &sample_transport(
                Some(5),
                Some(4),
                Some(json!({
                    "failover_rules": {
                        "max_retries": 2
                    }
                })),
            ),
        );

        assert_eq!(
            identities,
            vec![
                ExecutionAttemptIdentity::new(1, 0),
                ExecutionAttemptIdentity::new(1, 1),
            ]
        );
    }

    #[test]
    fn build_local_attempt_identities_falls_back_to_endpoint_max_retries() {
        let identities =
            build_local_attempt_identities(2, &sample_transport(Some(5), Some(3), None));

        assert_eq!(
            identities,
            vec![
                ExecutionAttemptIdentity::new(2, 0),
                ExecutionAttemptIdentity::new(2, 1),
                ExecutionAttemptIdentity::new(2, 2),
            ]
        );
    }

    #[test]
    fn build_local_attempt_identities_falls_back_to_provider_max_retries() {
        let identities = build_local_attempt_identities(0, &sample_transport(Some(4), None, None));

        assert_eq!(
            identities,
            vec![
                ExecutionAttemptIdentity::new(0, 0),
                ExecutionAttemptIdentity::new(0, 1),
                ExecutionAttemptIdentity::new(0, 2),
                ExecutionAttemptIdentity::new(0, 3),
            ]
        );
    }

    #[test]
    fn build_local_attempt_identities_endpoint_overrides_provider() {
        let identities =
            build_local_attempt_identities(7, &sample_transport(Some(10), Some(3), None));

        assert_eq!(
            identities,
            vec![
                ExecutionAttemptIdentity::new(7, 0),
                ExecutionAttemptIdentity::new(7, 1),
                ExecutionAttemptIdentity::new(7, 2),
            ]
        );
    }

    #[test]
    fn build_local_attempt_identities_default_two_treated_as_unset() {
        let identities =
            build_local_attempt_identities(5, &sample_transport(Some(2), Some(2), None));

        assert_eq!(identities, vec![ExecutionAttemptIdentity::new(5, 0)]);
    }

    #[test]
    fn build_local_attempt_identities_endpoint_two_falls_back_to_provider_ten() {
        let identities =
            build_local_attempt_identities(1, &sample_transport(Some(10), Some(2), None));

        assert_eq!(
            identities,
            vec![
                ExecutionAttemptIdentity::new(1, 0),
                ExecutionAttemptIdentity::new(1, 1),
                ExecutionAttemptIdentity::new(1, 2),
                ExecutionAttemptIdentity::new(1, 3),
                ExecutionAttemptIdentity::new(1, 4),
                ExecutionAttemptIdentity::new(1, 5),
                ExecutionAttemptIdentity::new(1, 6),
                ExecutionAttemptIdentity::new(1, 7),
                ExecutionAttemptIdentity::new(1, 8),
                ExecutionAttemptIdentity::new(1, 9),
            ]
        );
    }

    #[test]
    fn build_local_attempt_identities_failover_rules_zero_produces_one_slot() {
        let identities = build_local_attempt_identities(
            1,
            &sample_transport(
                Some(5),
                Some(4),
                Some(json!({
                    "failover_rules": {
                        "max_retries": 0
                    }
                })),
            ),
        );

        assert_eq!(identities, vec![ExecutionAttemptIdentity::new(1, 0)]);
    }

    #[test]
    fn build_local_attempt_identities_endpoint_zero_produces_one_slot() {
        let identities =
            build_local_attempt_identities(3, &sample_transport(Some(5), Some(0), None));

        assert_eq!(identities, vec![ExecutionAttemptIdentity::new(3, 0)]);
    }

    #[test]
    fn build_local_attempt_identities_provider_zero_produces_one_slot() {
        let identities = build_local_attempt_identities(3, &sample_transport(Some(0), None, None));

        assert_eq!(identities, vec![ExecutionAttemptIdentity::new(3, 0)]);
    }

    #[test]
    fn build_local_attempt_identities_provider_ten_creates_ten_slots() {
        let identities = build_local_attempt_identities(2, &sample_transport(Some(10), None, None));

        assert_eq!(identities.len(), 10);
        assert_eq!(identities[0], ExecutionAttemptIdentity::new(2, 0));
        assert_eq!(identities[9], ExecutionAttemptIdentity::new(2, 9));
    }

    #[test]
    fn build_local_attempt_identities_failover_rules_over_limit_clamped_to_max() {
        let identities = build_local_attempt_identities(
            0,
            &sample_transport(
                Some(3),
                Some(5),
                Some(json!({
                    "failover_rules": {
                        "max_retries": 1000
                    }
                })),
            ),
        );

        assert_eq!(identities.len(), 99);
    }

    #[test]
    fn build_local_attempt_identities_failover_rules_u32_max_clamped_to_max() {
        let identities = build_local_attempt_identities(
            0,
            &sample_transport(
                None,
                None,
                Some(json!({
                    "failover_rules": {
                        "max_retries": u32::MAX
                    }
                })),
            ),
        );

        assert_eq!(identities.len(), 99);
    }

    #[test]
    fn build_local_attempt_identities_endpoint_over_limit_clamped_to_max() {
        let identities =
            build_local_attempt_identities(0, &sample_transport(None, Some(2000), None));

        assert_eq!(identities.len(), 99);
    }

    #[test]
    fn build_local_attempt_identities_provider_over_limit_clamped_to_max() {
        let identities =
            build_local_attempt_identities(0, &sample_transport(Some(5000), None, None));

        assert_eq!(identities.len(), 99);
    }

    #[test]
    fn parse_attempt_identity_from_report_context_reads_candidate_and_retry_indices() {
        let identity = attempt_identity_from_report_context(Some(&json!({
            "candidate_index": 4,
            "retry_index": 1,
            "pool_key_index": 7,
        })))
        .expect("attempt identity should parse");

        assert_eq!(
            identity,
            ExecutionAttemptIdentity {
                candidate_index: 4,
                retry_index: 1,
                pool_key_index: Some(7),
            }
        );
    }

    #[test]
    fn parse_candidate_metadata_from_report_context_reads_group_and_pool_metadata() {
        let metadata = local_execution_candidate_metadata_from_report_context(Some(&json!({
            "candidate_group_id": "group-1",
            "pool_key_index": 3,
        })));

        assert_eq!(
            metadata,
            LocalExecutionCandidateMetadata {
                candidate_group_id: Some("group-1".to_string()),
                pool_key_index: Some(3),
            }
        );
    }
}
