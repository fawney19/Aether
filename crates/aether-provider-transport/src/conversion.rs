use aether_ai_formats::normalize_api_format_alias;
use aether_ai_formats::protocol::matrix::{
    request_conversion_kind, request_conversion_requires_enable_flag, RequestConversionKind,
};

use crate::auth::{
    resolve_local_gemini_auth, resolve_local_openai_bearer_auth, resolve_local_standard_auth,
};
use crate::kiro::{
    is_kiro_claude_messages_transport, local_kiro_request_transport_unsupported_reason_with_network,
};
use crate::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_openai_chat_transport_unsupported_reason,
    local_standard_transport_unsupported_reason_with_network,
};
use crate::vertex::{
    is_vertex_api_key_transport_context,
    local_vertex_api_key_gemini_transport_unsupported_reason_with_network,
    resolve_local_vertex_api_key_query_auth, VERTEX_API_KEY_QUERY_PARAM,
};
use crate::GatewayProviderTransportSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateTransportPolicyFacts<'a> {
    pub endpoint_api_format: &'a str,
    pub global_model_name: &'a str,
    pub selected_provider_model_name: &'a str,
    pub mapping_matched_model: Option<&'a str>,
}

pub fn request_conversion_enabled_for_transport(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = normalize_api_format_alias(client_api_format);
    let provider_api_format = normalize_api_format_alias(provider_api_format);
    if client_api_format == provider_api_format {
        return true;
    }
    if request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str()).is_none() {
        return false;
    }
    if !request_conversion_requires_enable_flag(
        client_api_format.as_str(),
        provider_api_format.as_str(),
    ) {
        return true;
    }
    transport.provider.enable_format_conversion
        || endpoint_accepts_client_api_format(transport, client_api_format.as_str())
}

pub fn request_pair_allowed_for_transport(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> bool {
    let client_api_format = normalize_api_format_alias(client_api_format);
    let provider_api_format = normalize_api_format_alias(provider_api_format);
    if client_api_format == provider_api_format {
        return true;
    }
    if request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str()).is_none() {
        return false;
    }
    if is_kiro_claude_messages_transport(transport, &provider_api_format) {
        return request_conversion_enabled_for_transport(
            transport,
            client_api_format.as_str(),
            provider_api_format.as_str(),
        ) && local_kiro_request_transport_unsupported_reason_with_network(transport)
            .is_none();
    }
    request_conversion_enabled_for_transport(
        transport,
        client_api_format.as_str(),
        provider_api_format.as_str(),
    )
}

pub fn request_conversion_transport_supported(
    transport: &GatewayProviderTransportSnapshot,
    kind: RequestConversionKind,
) -> bool {
    request_conversion_transport_unsupported_reason(transport, kind).is_none()
}

pub fn request_conversion_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    _kind: RequestConversionKind,
) -> Option<&'static str> {
    if is_kiro_claude_messages_transport(transport, &transport.endpoint.api_format) {
        return local_kiro_request_transport_unsupported_reason_with_network(transport);
    }

    match normalize_api_format_alias(&transport.endpoint.api_format).as_str() {
        "openai:chat" => local_openai_chat_transport_unsupported_reason(transport),
        "openai:responses" | "openai:responses:compact" => {
            local_standard_transport_unsupported_reason_with_network(
                transport,
                transport.endpoint.api_format.trim(),
            )
        }
        "claude:messages" => {
            local_standard_transport_unsupported_reason_with_network(transport, "claude:messages")
        }
        "gemini:generate_content" if is_vertex_api_key_transport_context(transport) => {
            local_vertex_api_key_gemini_transport_unsupported_reason_with_network(transport)
        }
        "gemini:generate_content" => local_gemini_transport_unsupported_reason_with_network(
            transport,
            "gemini:generate_content",
        ),
        _ => Some("transport_api_format_unsupported"),
    }
}

pub fn request_conversion_direct_auth(
    transport: &GatewayProviderTransportSnapshot,
    _kind: RequestConversionKind,
) -> Option<(String, String)> {
    match normalize_api_format_alias(&transport.endpoint.api_format).as_str() {
        "openai:chat" | "openai:responses" | "openai:responses:compact" => {
            resolve_local_openai_bearer_auth(transport)
        }
        "gemini:generate_content" => {
            if is_vertex_api_key_transport_context(transport) {
                resolve_local_vertex_api_key_query_auth(transport)
                    .map(|auth| (VERTEX_API_KEY_QUERY_PARAM.to_string(), auth.value))
            } else {
                resolve_local_gemini_auth(transport)
            }
        }
        "claude:messages" => resolve_local_standard_auth(transport),
        _ => None,
    }
}

pub fn candidate_common_transport_skip_reason(
    transport: &GatewayProviderTransportSnapshot,
    candidate: CandidateTransportPolicyFacts<'_>,
    requested_model: Option<&str>,
) -> Option<&'static str> {
    let requested_model = requested_model.unwrap_or_default();

    if !transport.provider.is_active {
        return Some("provider_inactive");
    }
    if !transport.endpoint.is_active {
        return Some("endpoint_inactive");
    }
    if !transport.key.is_active {
        return Some("key_inactive");
    }

    let endpoint_api_format = transport.endpoint.api_format.trim();
    if !aether_ai_formats::api_format_alias_matches(
        candidate.endpoint_api_format,
        endpoint_api_format,
    ) {
        return Some("endpoint_api_format_changed");
    }

    if !transport_key_supports_api_format(transport, endpoint_api_format) {
        return Some("key_api_format_disabled");
    }
    if !transport_key_allows_candidate_model(transport, requested_model, candidate) {
        return Some("key_model_disabled");
    }

    None
}

pub fn candidate_transport_pair_skip_reason(
    transport: &GatewayProviderTransportSnapshot,
    normalized_client_api_format: &str,
) -> Option<&'static str> {
    let endpoint_api_format = transport.endpoint.api_format.trim();
    if aether_ai_formats::api_format_alias_matches(
        endpoint_api_format,
        normalized_client_api_format,
    ) {
        return None;
    }

    if let Some(skip_reason) =
        disabled_format_conversion_skip_reason(transport, normalized_client_api_format)
    {
        return Some(skip_reason);
    }

    if !request_pair_allowed_for_transport(
        transport,
        normalized_client_api_format,
        endpoint_api_format,
    ) {
        return Some("transport_unsupported");
    }

    None
}

fn disabled_format_conversion_skip_reason(
    transport: &GatewayProviderTransportSnapshot,
    normalized_client_api_format: &str,
) -> Option<&'static str> {
    let endpoint_api_format = transport.endpoint.api_format.trim();
    if aether_ai_formats::api_format_alias_matches(
        endpoint_api_format,
        normalized_client_api_format,
    ) {
        return None;
    }

    request_conversion_kind(normalized_client_api_format, endpoint_api_format)?;

    if request_conversion_requires_enable_flag(normalized_client_api_format, endpoint_api_format)
        && !request_conversion_enabled_for_transport(
            transport,
            normalized_client_api_format,
            endpoint_api_format,
        )
    {
        return Some("format_conversion_disabled");
    }

    None
}

fn transport_key_supports_api_format(
    transport: &GatewayProviderTransportSnapshot,
    endpoint_api_format: &str,
) -> bool {
    let inherits_provider_api_formats =
        crate::provider_types::fixed_provider_key_inherits_api_formats(
            transport.provider.provider_type.as_str(),
            transport.key.auth_type.as_str(),
            transport.key.decrypted_auth_config.as_deref(),
        );
    if inherits_provider_api_formats {
        return true;
    }

    match transport.key.api_formats.as_deref() {
        None => true,
        Some(formats) => formats
            .iter()
            .any(|value| aether_ai_formats::api_format_alias_matches(value, endpoint_api_format)),
    }
}

fn transport_key_allows_candidate_model(
    transport: &GatewayProviderTransportSnapshot,
    requested_model: &str,
    candidate: CandidateTransportPolicyFacts<'_>,
) -> bool {
    let Some(allowed_models) = transport.key.allowed_models.as_deref() else {
        return true;
    };

    let requested_model = requested_model.trim();
    let global_model_name = candidate.global_model_name.trim();
    let selected_provider_model_name = candidate.selected_provider_model_name.trim();
    let mapping_matched_model = candidate
        .mapping_matched_model
        .map(str::trim)
        .filter(|value| !value.is_empty());

    for allowed_model in allowed_models.iter().map(String::as_str).map(str::trim) {
        if allowed_model.is_empty() {
            continue;
        }
        if allowed_model == requested_model
            || allowed_model == global_model_name
            || allowed_model == selected_provider_model_name
            || mapping_matched_model.is_some_and(|value| value == allowed_model)
        {
            return true;
        }
    }

    false
}

fn endpoint_accepts_client_api_format(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
) -> bool {
    let Some(config) = transport
        .endpoint
        .format_acceptance_config
        .as_ref()
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    if !config
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }

    if config
        .get("reject_formats")
        .is_some_and(|value| json_format_list_contains(value, client_api_format))
    {
        return false;
    }

    match config.get("accept_formats") {
        Some(value) => json_format_list_contains(value, client_api_format),
        None => true,
    }
}

fn json_format_list_contains(value: &serde_json::Value, api_format: &str) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    items.iter().any(|item| {
        item.as_str().is_some_and(|candidate| {
            aether_ai_formats::api_format_alias_matches(candidate, api_format)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_common_transport_skip_reason, candidate_transport_pair_skip_reason,
        request_conversion_direct_auth, request_conversion_enabled_for_transport,
        request_conversion_transport_supported, request_pair_allowed_for_transport,
        CandidateTransportPolicyFacts,
    };
    use aether_ai_formats::protocol::matrix::RequestConversionKind;
    use serde_json::json;

    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn transport_snapshot(
        provider_type: &str,
        api_format: &str,
        auth_type: &str,
        enable_format_conversion: bool,
        format_acceptance_config: Option<serde_json::Value>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: format!("provider-{provider_type}"),
                name: provider_type.to_string(),
                provider_type: provider_type.to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: format!("provider-{provider_type}"),
                api_format: api_format.to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: if provider_type == "kiro" {
                    "https://q.{region}.amazonaws.com".to_string()
                } else if provider_type == "vertex_ai" {
                    "https://aiplatform.googleapis.com".to_string()
                } else {
                    "https://api.example.com".to_string()
                },
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: format!("provider-{provider_type}"),
                name: "key".to_string(),
                auth_type: auth_type.to_string(),
                is_active: true,
                api_formats: Some(vec![api_format.to_string()]),
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

    fn candidate_facts(api_format: &str) -> CandidateTransportPolicyFacts<'_> {
        CandidateTransportPolicyFacts {
            endpoint_api_format: api_format,
            global_model_name: "gpt-4.1",
            selected_provider_model_name: "provider-gpt-4.1",
            mapping_matched_model: Some("gpt-4.1-mini"),
        }
    }

    #[test]
    fn conversion_helpers_follow_transport_api_format() {
        let transport = transport_snapshot("openai", "openai:chat", "bearer", true, None);

        assert!(request_conversion_transport_supported(
            &transport,
            RequestConversionKind::ToOpenAIChat
        ));
        assert_eq!(
            request_conversion_direct_auth(&transport, RequestConversionKind::ToOpenAIChat),
            Some(("authorization".to_string(), "Bearer secret".to_string()))
        );
    }

    #[test]
    fn endpoint_level_format_acceptance_enables_cross_format_pair_without_provider_flag() {
        let transport = transport_snapshot(
            "custom",
            "openai:responses",
            "bearer",
            false,
            Some(json!({
                "enabled": true,
                "accept_formats": ["claude:messages"],
            })),
        );

        assert!(request_conversion_enabled_for_transport(
            &transport,
            "claude:messages",
            "openai:responses"
        ));
        assert!(request_pair_allowed_for_transport(
            &transport,
            "claude:messages",
            "openai:responses"
        ));
        assert!(!request_pair_allowed_for_transport(
            &transport,
            "gemini:generate_content",
            "openai:responses"
        ));
    }

    #[test]
    fn endpoint_reject_formats_override_endpoint_cross_format_enablement() {
        let transport = transport_snapshot(
            "custom",
            "openai:responses",
            "bearer",
            false,
            Some(json!({
                "enabled": true,
                "reject_formats": ["claude:messages"],
            })),
        );

        assert!(!request_conversion_enabled_for_transport(
            &transport,
            "claude:messages",
            "openai:responses"
        ));
    }

    #[test]
    fn vertex_gemini_transport_supports_cross_format_conversion_with_query_auth() {
        let transport = transport_snapshot(
            "vertex_ai",
            "gemini:generate_content",
            "api_key",
            true,
            None,
        );

        assert!(request_conversion_transport_supported(
            &transport,
            RequestConversionKind::ToGeminiStandard
        ));
        assert_eq!(
            request_conversion_direct_auth(&transport, RequestConversionKind::ToGeminiStandard),
            Some(("key".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn kiro_claude_messages_transport_supports_cross_format_conversion_via_envelope() {
        let transport = transport_snapshot("kiro", "claude:messages", "bearer", true, None);

        assert!(request_pair_allowed_for_transport(
            &transport,
            "openai:chat",
            "claude:messages"
        ));
        assert!(request_conversion_transport_supported(
            &transport,
            RequestConversionKind::ToClaudeStandard
        ));
    }

    #[test]
    fn candidate_common_transport_policy_checks_active_state_format_and_allowed_models() {
        let mut transport = transport_snapshot("custom", "openai:chat", "bearer", true, None);
        transport.key.allowed_models = Some(vec!["gpt-4.1-mini".to_string()]);

        assert_eq!(
            candidate_common_transport_skip_reason(
                &transport,
                candidate_facts("openai:chat"),
                Some("gpt-4.1")
            ),
            None
        );

        assert_eq!(
            candidate_common_transport_skip_reason(
                &transport,
                candidate_facts("claude:messages"),
                Some("gpt-4.1")
            ),
            Some("endpoint_api_format_changed")
        );

        transport.key.allowed_models = Some(vec!["other-model".to_string()]);
        assert_eq!(
            candidate_common_transport_skip_reason(
                &transport,
                candidate_facts("openai:chat"),
                Some("gpt-4.1")
            ),
            Some("key_model_disabled")
        );
    }

    #[test]
    fn fixed_provider_oauth_keys_inherit_endpoint_api_formats_for_candidate_policy() {
        let mut transport = transport_snapshot("codex", "openai:responses", "oauth", true, None);
        transport.key.api_formats = Some(vec!["openai:image".to_string()]);

        assert_eq!(
            candidate_common_transport_skip_reason(
                &transport,
                candidate_facts("openai:responses"),
                None,
            ),
            None
        );
    }

    #[test]
    fn candidate_transport_pair_policy_reports_disabled_conversion_and_unsupported_pairs() {
        let transport = transport_snapshot("custom", "openai:responses", "bearer", false, None);

        assert_eq!(
            candidate_transport_pair_skip_reason(&transport, "openai:chat"),
            Some("format_conversion_disabled")
        );
        assert_eq!(
            candidate_transport_pair_skip_reason(&transport, "gemini:video"),
            Some("transport_unsupported")
        );

        let enabled_transport =
            transport_snapshot("custom", "openai:responses", "bearer", true, None);
        assert_eq!(
            candidate_transport_pair_skip_reason(&enabled_transport, "openai:chat"),
            None
        );
    }
}
