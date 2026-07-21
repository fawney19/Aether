#[path = "plans/candidates.rs"]
mod candidates;
#[path = "plans/diagnostic.rs"]
mod diagnostic;
#[path = "plans/resolve.rs"]
mod resolve;
#[path = "plans/stream.rs"]
mod stream;
#[path = "plans/sync.rs"]
mod sync;

use crate::ai_serving::planner::common::resolve_upstream_is_stream_for_provider;
use crate::ai_serving::GatewayProviderTransportSnapshot;

pub(super) use self::candidates::list_local_openai_chat_candidates;
pub(super) use self::diagnostic::set_local_openai_chat_miss_diagnostic;
pub(super) use self::resolve::resolve_local_openai_chat_decision_input;
pub(super) use self::stream::{
    build_local_openai_chat_stream_attempt_source, build_local_openai_chat_stream_plan_and_reports,
};
pub(super) use self::sync::{
    build_local_openai_chat_sync_attempt_source, build_local_openai_chat_sync_plan_and_reports,
};

pub(super) fn openai_chat_upstream_is_stream_for_candidate(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    client_is_stream: bool,
) -> bool {
    let hard_requires_streaming =
        crate::ai_serving::transport::kiro::is_kiro_claude_messages_transport(
            transport,
            provider_api_format,
        ) || openai_chat_antigravity_requires_upstream_streaming(transport, provider_api_format)
            || openai_chat_gemini_cli_client_stream_requires_upstream_streaming(
                transport,
                provider_api_format,
                client_is_stream,
            );
    resolve_upstream_is_stream_for_provider(
        transport.endpoint.config.as_ref(),
        transport.provider.provider_type.as_str(),
        provider_api_format,
        client_is_stream,
        hard_requires_streaming,
    )
}

fn openai_chat_antigravity_requires_upstream_streaming(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> bool {
    crate::ai_serving::transport::antigravity::is_antigravity_provider_transport(transport)
        && crate::ai_serving::normalize_api_format_alias(provider_api_format)
            == "gemini:generate_content"
}

fn openai_chat_gemini_cli_client_stream_requires_upstream_streaming(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
    client_is_stream: bool,
) -> bool {
    crate::ai_serving::transport::gemini_cli::is_gemini_cli_provider_transport(transport)
        && crate::ai_serving::transport::gemini_cli::gemini_cli_v1internal_requires_upstream_streaming(
            provider_api_format,
            client_is_stream,
        )
}

#[cfg(test)]
mod tests {
    use super::openai_chat_upstream_is_stream_for_candidate;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use serde_json::{json, Value};

    fn sample_transport(
        provider_type: &str,
        api_format: &str,
        endpoint_config: Option<Value>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
                provider_type: provider_type.to_string(),
                website: None,
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
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: api_format.to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.example.test".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: endpoint_config,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
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
                upstream_metadata: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn openai_chat_policy_resolver_supports_force_stream_force_non_stream_and_auto() {
        let force_stream = sample_transport(
            "openai",
            "openai:chat",
            Some(json!({"upstream_stream_policy": "force_stream"})),
        );
        assert!(openai_chat_upstream_is_stream_for_candidate(
            &force_stream,
            "openai:chat",
            false,
        ));

        let force_non_stream = sample_transport(
            "openai",
            "openai:chat",
            Some(json!({"upstream_stream_policy": "force_non_stream"})),
        );
        assert!(!openai_chat_upstream_is_stream_for_candidate(
            &force_non_stream,
            "openai:chat",
            true,
        ));

        let auto = sample_transport(
            "openai",
            "openai:chat",
            Some(json!({"upstream_stream_policy": "auto"})),
        );
        assert!(openai_chat_upstream_is_stream_for_candidate(
            &auto,
            "openai:chat",
            true,
        ));
        assert!(!openai_chat_upstream_is_stream_for_candidate(
            &auto,
            "openai:chat",
            false,
        ));
    }

    #[test]
    fn openai_chat_policy_resolver_preserves_provider_hard_streaming() {
        let codex = sample_transport(
            "codex",
            "openai:responses",
            Some(json!({"upstream_stream_policy": "force_non_stream"})),
        );

        assert!(openai_chat_upstream_is_stream_for_candidate(
            &codex,
            "openai:responses",
            false,
        ));
    }

    #[test]
    fn openai_chat_policy_resolver_preserves_gemini_cli_streaming_requests() {
        let gemini_cli = sample_transport(
            "gemini_cli",
            "gemini:generate_content",
            Some(json!({"upstream_stream_policy": "force_non_stream"})),
        );

        assert!(openai_chat_upstream_is_stream_for_candidate(
            &gemini_cli,
            "gemini:generate_content",
            true,
        ));
        assert!(!openai_chat_upstream_is_stream_for_candidate(
            &gemini_cli,
            "gemini:generate_content",
            false,
        ));
    }

    #[test]
    fn openai_chat_policy_resolver_preserves_antigravity_streaming_envelope() {
        let antigravity = sample_transport(
            "antigravity",
            "gemini:generate_content",
            Some(json!({"upstream_stream_policy": "force_non_stream"})),
        );

        assert!(openai_chat_upstream_is_stream_for_candidate(
            &antigravity,
            "gemini:generate_content",
            false,
        ));
        assert!(openai_chat_upstream_is_stream_for_candidate(
            &antigravity,
            "gemini:generate_content",
            true,
        ));
    }
}
