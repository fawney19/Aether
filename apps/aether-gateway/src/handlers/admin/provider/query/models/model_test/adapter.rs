use super::DEFAULT_PROVIDER_QUERY_TEST_MESSAGE;
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::provider_transport::antigravity::{
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestAuthUnsupportedReason, AntigravityRequestSideSupport,
    AntigravityRequestSideUnsupportedReason,
};
use crate::provider_transport::kiro::supports_local_kiro_request_transport_with_network;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProviderQueryTestAdapter {
    Standard,
    Grok,
    Kiro,
    OpenAiImage,
    Antigravity,
}
pub(super) fn provider_query_unsupported_test_api_format_message(api_format: &str) -> String {
    let api_format = api_format.trim();
    if api_format.is_empty() {
        "Rust local provider-query model test does not support an empty endpoint format".to_string()
    } else {
        format!(
            "Rust local provider-query model test does not support endpoint format {api_format}"
        )
    }
}

pub(super) fn provider_query_standard_test_client_api_format(
    provider_api_format: &str,
) -> &'static str {
    let normalized_api_format = crate::ai_serving::normalize_api_format_alias(provider_api_format);
    if normalized_api_format == "openai:responses:compact" {
        "openai:responses:compact"
    } else if normalized_api_format == "openai:search" {
        "openai:search"
    } else if crate::ai_serving::is_embedding_api_format(&normalized_api_format) {
        "openai:embedding"
    } else if crate::ai_serving::is_rerank_api_format(&normalized_api_format) {
        "openai:rerank"
    } else {
        "openai:chat"
    }
}

pub(super) fn provider_query_standard_test_unsupported_reason(
    transport: &AdminGatewayProviderTransportSnapshot,
    api_format: &str,
) -> String {
    let normalized_api_format = crate::ai_serving::normalize_api_format_alias(api_format);
    if crate::provider_transport::is_windsurf_provider_transport(transport)
        && normalized_api_format == "openai:chat"
    {
        let reason =
            crate::provider_transport::local_windsurf_request_transport_unsupported_reason_with_network(
                transport,
            );
        return match reason {
            Some(reason) => format!(
                "{} ({reason})",
                provider_query_unsupported_test_api_format_message(api_format)
            ),
            None => provider_query_unsupported_test_api_format_message(api_format),
        };
    }

    let reason = match normalized_api_format.as_str() {
        "openai:chat" => {
            crate::provider_transport::policy::local_openai_chat_transport_unsupported_reason(
                transport,
            )
        }
        "openai:responses"
        | "openai:responses:compact"
        | "openai:search"
        | "claude:messages"
        | "openai:embedding"
        | "jina:embedding"
        | "doubao:embedding"
        | "aliyun:multimodal_embedding"
        | "openai:rerank"
        | "jina:rerank" => {
            crate::provider_transport::policy::local_standard_transport_unsupported_reason_with_network(
                transport,
                api_format,
            )
        }
        "gemini:generate_content" | "gemini:embedding" | "gemini:interactions"
            if crate::provider_transport::is_vertex_transport_context(transport) =>
        {
            aether_provider_transport::vertex::local_vertex_gemini_transport_unsupported_reason_with_network(
                transport,
            )
        }
        "gemini:generate_content" | "gemini:embedding" | "gemini:interactions" => {
            crate::provider_transport::policy::local_gemini_transport_unsupported_reason_with_network(
                transport,
                api_format,
            )
        }
        _ => Some("transport_api_format_mismatch"),
    };

    match reason {
        Some(reason) => format!(
            "{} ({reason})",
            provider_query_unsupported_test_api_format_message(api_format)
        ),
        None => provider_query_unsupported_test_api_format_message(api_format),
    }
}

pub(super) fn provider_query_antigravity_unsupported_reason(
    reason: AntigravityRequestSideUnsupportedReason,
) -> &'static str {
    match reason {
        AntigravityRequestSideUnsupportedReason::InactiveTransport => "transport_inactive",
        AntigravityRequestSideUnsupportedReason::WrongProviderType => {
            "transport_provider_type_unsupported"
        }
        AntigravityRequestSideUnsupportedReason::UnsupportedApiFormat => {
            "transport_api_format_mismatch"
        }
        AntigravityRequestSideUnsupportedReason::UnsupportedCustomPath => {
            "transport_custom_path_unsupported"
        }
        AntigravityRequestSideUnsupportedReason::UnsupportedHeaderRules => {
            "transport_header_rules_unsupported"
        }
        AntigravityRequestSideUnsupportedReason::UnsupportedBodyRules => {
            "transport_body_rules_unsupported"
        }
        AntigravityRequestSideUnsupportedReason::UnsupportedNetworkConfig => {
            "transport_network_config_unsupported"
        }
        AntigravityRequestSideUnsupportedReason::UnsupportedAuth(
            AntigravityRequestAuthUnsupportedReason::WrongProviderType,
        ) => "transport_provider_type_unsupported",
        AntigravityRequestSideUnsupportedReason::UnsupportedAuth(
            AntigravityRequestAuthUnsupportedReason::MissingAuthConfig,
        ) => "transport_antigravity_auth_config_missing",
        AntigravityRequestSideUnsupportedReason::UnsupportedAuth(
            AntigravityRequestAuthUnsupportedReason::InvalidAuthConfigJson,
        ) => "transport_antigravity_auth_config_invalid",
        AntigravityRequestSideUnsupportedReason::UnsupportedAuth(
            AntigravityRequestAuthUnsupportedReason::ComplexDynamicAuthConfig,
        ) => "transport_antigravity_auth_config_unsupported",
        AntigravityRequestSideUnsupportedReason::UnsupportedAuth(
            AntigravityRequestAuthUnsupportedReason::MissingProjectId,
        ) => "transport_antigravity_project_id_missing",
        AntigravityRequestSideUnsupportedReason::UnsupportedEnvelope(_) => {
            "transport_antigravity_envelope_unsupported"
        }
    }
}

pub(super) fn provider_query_antigravity_test_unsupported_reason(
    transport: &AdminGatewayProviderTransportSnapshot,
    request_body: &Value,
) -> Option<&'static str> {
    match classify_local_antigravity_request_support(
        transport,
        request_body,
        AntigravityEnvelopeRequestType::EndpointTest,
    ) {
        AntigravityRequestSideSupport::Supported(_) => None,
        AntigravityRequestSideSupport::Unsupported(reason) => {
            Some(provider_query_antigravity_unsupported_reason(reason))
        }
    }
}

pub(super) fn provider_query_grok_test_unsupported_reason(
    transport: &AdminGatewayProviderTransportSnapshot,
    api_format: &str,
) -> Option<&'static str> {
    if !transport.provider.is_active {
        return Some("provider_inactive");
    }
    if !transport.endpoint.is_active {
        return Some("endpoint_inactive");
    }
    if !transport.key.is_active {
        return Some("key_inactive");
    }
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok")
    {
        return Some("transport_provider_type_unsupported");
    }
    let normalized_api_format = provider_query_normalize_api_format_alias(api_format);
    if !matches!(
        normalized_api_format.as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages"
    ) {
        return Some("transport_api_format_mismatch");
    }
    if provider_query_normalize_api_format_alias(&transport.endpoint.api_format)
        != normalized_api_format
    {
        return Some("transport_api_format_mismatch");
    }
    if crate::provider_transport::resolve_grok_session_auth(transport).is_none() {
        return Some("transport_oauth_resolution_unsupported");
    }
    if !crate::provider_transport::header_rules_are_locally_supported(
        transport.endpoint.header_rules.as_ref(),
    ) {
        return Some("transport_header_rules_unsupported");
    }
    if !crate::provider_transport::body_rules_are_locally_supported(
        transport.endpoint.body_rules.as_ref(),
    ) {
        return Some("transport_body_rules_unsupported");
    }
    if !crate::provider_transport::transport_proxy_is_locally_supported(transport) {
        return Some("transport_proxy_unsupported");
    }
    if crate::provider_transport::transport_profile_is_configured(transport)
        && crate::provider_transport::resolve_transport_profile(transport).is_none()
    {
        return Some("transport_profile_unsupported");
    }

    None
}

pub(super) fn provider_query_normalize_api_format_alias(value: &str) -> String {
    crate::ai_serving::normalize_api_format_alias(value)
}

pub(super) fn provider_query_test_adapter_for_provider_api_format(
    provider_type: &str,
    api_format: &str,
) -> Option<ProviderQueryTestAdapter> {
    if provider_type.trim().eq_ignore_ascii_case("kiro") {
        return Some(ProviderQueryTestAdapter::Kiro);
    }

    let normalized_api_format = provider_query_normalize_api_format_alias(api_format);
    if provider_type.trim().eq_ignore_ascii_case("grok") {
        return match normalized_api_format.as_str() {
            "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages" => {
                Some(ProviderQueryTestAdapter::Grok)
            }
            "openai:image" => Some(ProviderQueryTestAdapter::OpenAiImage),
            _ => None,
        };
    }
    if normalized_api_format == "openai:image" {
        return Some(ProviderQueryTestAdapter::OpenAiImage);
    }
    if provider_type.trim().eq_ignore_ascii_case("antigravity")
        && normalized_api_format == "gemini:generate_content"
    {
        return Some(ProviderQueryTestAdapter::Antigravity);
    }
    if matches!(
        normalized_api_format.as_str(),
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "openai:search"
            | "claude:messages"
            | "gemini:generate_content"
            | "gemini:interactions"
            | "openai:embedding"
            | "gemini:embedding"
            | "jina:embedding"
            | "doubao:embedding"
            | "aliyun:multimodal_embedding"
            | "openai:rerank"
            | "jina:rerank"
    ) {
        return Some(ProviderQueryTestAdapter::Standard);
    }

    None
}

pub(super) fn provider_query_model_test_endpoint_priority(
    provider_type: &str,
    api_format: &str,
) -> Option<u8> {
    let normalized_api_format = provider_query_normalize_api_format_alias(api_format);
    match provider_query_test_adapter_for_provider_api_format(provider_type, api_format)? {
        ProviderQueryTestAdapter::Kiro => Some(0),
        ProviderQueryTestAdapter::Grok => {
            if matches!(
                normalized_api_format.as_str(),
                "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages"
            ) {
                Some(0)
            } else {
                Some(2)
            }
        }
        ProviderQueryTestAdapter::Antigravity => Some(1),
        ProviderQueryTestAdapter::OpenAiImage => Some(2),
        ProviderQueryTestAdapter::Standard => {
            if matches!(
                normalized_api_format.as_str(),
                "openai:chat" | "claude:messages" | "gemini:generate_content"
            ) {
                Some(0)
            } else {
                Some(1)
            }
        }
    }
}

pub(super) fn provider_query_default_antigravity_endpoint_test_body() -> Value {
    json!({
        "contents": [{
            "role": "user",
            "parts": [{
                "text": DEFAULT_PROVIDER_QUERY_TEST_MESSAGE
            }]
        }]
    })
}

pub(super) fn provider_query_transport_supports_model_test_execution(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    if crate::provider_transport::is_windsurf_provider_transport(transport)
        && provider_query_normalize_api_format_alias(api_format) == "openai:chat"
    {
        return crate::provider_transport::local_windsurf_request_transport_unsupported_reason_with_network(
            transport,
        )
        .is_none();
    }

    match provider_query_test_adapter_for_provider_api_format(
        transport.provider.provider_type.as_str(),
        api_format,
    ) {
        Some(ProviderQueryTestAdapter::Kiro) => {
            supports_local_kiro_request_transport_with_network(transport)
        }
        Some(ProviderQueryTestAdapter::OpenAiImage) => {
            crate::provider_transport::openai_image_transport_unsupported_reason(
                transport,
                "openai:image",
            )
            .is_none()
        }
        Some(ProviderQueryTestAdapter::Antigravity) => {
            provider_query_antigravity_test_unsupported_reason(
                transport,
                &provider_query_default_antigravity_endpoint_test_body(),
            )
            .is_none()
        }
        Some(ProviderQueryTestAdapter::Grok) => {
            provider_query_grok_test_unsupported_reason(transport, api_format).is_none()
        }
        Some(ProviderQueryTestAdapter::Standard) => match crate::ai_serving::normalize_api_format_alias(api_format).as_str() {
        "openai:chat" => {
            crate::provider_transport::policy::supports_local_openai_chat_transport(transport)
        }
        "openai:responses"
        | "openai:responses:compact"
        | "openai:search"
        | "openai:embedding"
        | "jina:embedding"
        | "doubao:embedding"
        | "aliyun:multimodal_embedding"
        | "openai:rerank"
        | "jina:rerank" => {
            crate::provider_transport::policy::supports_local_standard_transport_with_network(
                transport, api_format,
            )
        }
        "claude:messages" => {
            crate::provider_transport::policy::supports_local_standard_transport_with_network(
                transport, api_format,
            )
        }
        "gemini:generate_content" | "gemini:embedding" | "gemini:interactions" => {
            if crate::provider_transport::is_vertex_transport_context(transport) {
                aether_provider_transport::vertex::supports_local_vertex_gemini_transport_with_network(transport)
            } else {
                state.supports_local_gemini_transport_with_network(transport, api_format)
            }
        }
        _ => false,
    },
        None => false,
    }
}
