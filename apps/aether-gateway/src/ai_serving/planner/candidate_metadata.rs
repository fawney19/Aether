use aether_ai_serving::{
    append_ai_execution_contract_fields_to_value, append_ai_ranking_metadata_to_object,
    build_ai_candidate_metadata_from_candidate,
};
use aether_scheduler_core::{SchedulerMinimalCandidateSelectionCandidate, SchedulerRankingOutcome};
use serde_json::{Map, Value};

use crate::ai_serving::planner::candidate_resolution::EligibleLocalExecutionCandidate;
use crate::ai_serving::transport::append_transport_diagnostics_to_value;
use crate::ai_serving::GatewayProviderTransportSnapshot;
use crate::ai_serving::{ConversionMode, ExecutionStrategy};

pub(crate) struct LocalExecutionCandidateMetadataParts<'a> {
    pub(crate) eligible: &'a EligibleLocalExecutionCandidate,
    pub(crate) provider_api_format: &'a str,
    pub(crate) client_api_format: &'a str,
    pub(crate) extra_fields: Map<String, Value>,
}

pub(crate) fn append_ranking_metadata_to_object(
    object: &mut Map<String, Value>,
    ranking: &SchedulerRankingOutcome,
) {
    append_ai_ranking_metadata_to_object(object, ranking);
}

pub(crate) fn build_local_execution_candidate_metadata(
    parts: LocalExecutionCandidateMetadataParts<'_>,
) -> Value {
    build_local_execution_candidate_metadata_for_candidate(
        &parts.eligible.candidate,
        Some(parts.eligible.transport.as_ref()),
        parts.provider_api_format,
        parts.client_api_format,
        parts.extra_fields,
    )
}

pub(crate) fn build_local_execution_candidate_metadata_for_candidate(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    transport: Option<&GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    client_api_format: &str,
    extra_fields: Map<String, Value>,
) -> Value {
    append_transport_diagnostics_to_value(
        build_ai_candidate_metadata_from_candidate(
            candidate,
            provider_api_format,
            client_api_format,
            extra_fields,
        ),
        transport,
        client_api_format,
        provider_api_format,
    )
}

pub(crate) fn build_local_execution_candidate_contract_metadata(
    parts: LocalExecutionCandidateMetadataParts<'_>,
    execution_strategy: ExecutionStrategy,
    conversion_mode: ConversionMode,
    provider_contract: &str,
) -> Value {
    append_ai_execution_contract_fields_to_value(
        build_local_execution_candidate_metadata_for_candidate(
            &parts.eligible.candidate,
            Some(parts.eligible.transport.as_ref()),
            parts.provider_api_format,
            parts.client_api_format,
            parts.extra_fields,
        ),
        execution_strategy.as_str(),
        conversion_mode.as_str(),
        parts.client_api_format,
        provider_contract,
    )
}

pub(crate) fn build_local_execution_candidate_contract_metadata_for_candidate(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    transport: Option<&GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    client_api_format: &str,
    extra_fields: Map<String, Value>,
    execution_strategy: ExecutionStrategy,
    conversion_mode: ConversionMode,
    provider_contract: &str,
) -> Value {
    append_ai_execution_contract_fields_to_value(
        build_local_execution_candidate_metadata_for_candidate(
            candidate,
            transport,
            provider_api_format,
            client_api_format,
            extra_fields,
        ),
        execution_strategy.as_str(),
        conversion_mode.as_str(),
        client_api_format,
        provider_contract,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        build_local_execution_candidate_contract_metadata_for_candidate,
        build_local_execution_candidate_metadata_for_candidate,
    };
    use crate::ai_serving::transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use crate::ai_serving::{ConversionMode, ExecutionStrategy, GatewayProviderTransportSnapshot};
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
    use serde_json::{json, Value};

    fn sample_candidate() -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "RightCode".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 22,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:responses".to_string(),
            key_id: "key-1".to_string(),
            key_name: "codex".to_string(),
            key_auth_type: "oauth".to_string(),
            key_internal_priority: 10,
            key_global_priority_for_format: None,
            key_capabilities: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-1".to_string(),
            global_model_name: "gpt-5.4".to_string(),
            selected_provider_model_name: "gpt-5.4".to_string(),
            mapping_matched_model: None,
        }
    }

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "RightCode".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: Some(json!({"enabled": true, "mode": "node", "node_id": "proxy-node-1"})),
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:responses".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1/responses".to_string()),
                config: None,
                format_acceptance_config: Some(json!({
                    "enabled": true,
                    "accept_formats": ["claude:messages"]
                })),
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "codex".to_string(),
                auth_type: "oauth".to_string(),
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
                fingerprint: Some(json!({
                    "tls_profile": "chrome_136",
                    "user_agent": "Mozilla/5.0"
                })),
                decrypted_api_key: "sk-test".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn sample_claude_code_transport_without_auth() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-cc-1".to_string(),
                name: "NekoCode".to_string(),
                provider_type: "claude_code".to_string(),
                website: Some("https://nekocode.ai".to_string()),
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-cc-1".to_string(),
                provider_id: "provider-cc-1".to_string(),
                api_format: "claude:messages".to_string(),
                api_family: Some("claude".to_string()),
                endpoint_kind: Some("cli".to_string()),
                is_active: true,
                base_url: "https://api.anthropic.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-cc-1".to_string(),
                provider_id: "provider-cc-1".to_string(),
                name: "CC-特价-0.4".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["claude:messages".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn candidate_contract_metadata_includes_transport_diagnostics() {
        let metadata = build_local_execution_candidate_contract_metadata_for_candidate(
            &sample_candidate(),
            Some(&sample_transport()),
            "openai:responses",
            "claude:messages",
            serde_json::Map::new(),
            ExecutionStrategy::LocalCrossFormat,
            ConversionMode::Bidirectional,
            "openai:responses",
        );

        assert_eq!(metadata["transport_diagnostics"]["provider_type"], "codex");
        assert_eq!(
            metadata["transport_diagnostics"]["fingerprint"]["tls_profile"],
            "chrome_136"
        );
        assert_eq!(
            metadata["transport_diagnostics"]["resolved_tls_profile"],
            "chrome_136"
        );
        assert_eq!(
            metadata["transport_diagnostics"]["request_pair"]["conversion_enabled"],
            Value::Bool(true)
        );
        assert!(
            metadata["transport_diagnostics"]["request_pair"]["transport_unsupported_reason"]
                .is_null()
        );
    }

    #[test]
    fn candidate_metadata_marks_missing_transport_snapshot() {
        let metadata = build_local_execution_candidate_metadata_for_candidate(
            &sample_candidate(),
            None,
            "openai:responses",
            "openai:responses",
            serde_json::Map::new(),
        );

        assert_eq!(
            metadata["transport_diagnostics"]["transport_snapshot_available"],
            Value::Bool(false)
        );
    }

    #[test]
    fn candidate_metadata_uses_same_format_provider_specific_transport_reason() {
        let metadata = build_local_execution_candidate_metadata_for_candidate(
            &sample_candidate(),
            Some(&sample_claude_code_transport_without_auth()),
            "claude:messages",
            "claude:messages",
            serde_json::Map::new(),
        );

        assert_eq!(
            metadata["transport_diagnostics"]["request_pair"]["transport_unsupported_reason"],
            Value::String("transport_auth_unavailable".to_string())
        );
    }
}
