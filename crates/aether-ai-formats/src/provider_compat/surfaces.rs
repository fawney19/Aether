pub const ANTIGRAVITY_PROVIDER_TYPE: &str = "antigravity";
pub const KIRO_PROVIDER_TYPE: &str = "kiro";
pub const KIRO_ENVELOPE_NAME: &str = "kiro:generateAssistantResponse";
pub const ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME: &str = "antigravity:v1internal";
pub const GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME: &str = "gemini_cli:v1internal";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAdaptationSurface {
    AntigravityGeminiChat,
    AntigravityGeminiCli,
    GeminiCliV1Internal,
    KiroClaudeCli,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderAdaptationDescriptor {
    pub surface: ProviderAdaptationSurface,
    pub provider_type: Option<&'static str>,
    pub envelope_name: &'static str,
    pub anchor_api_format: &'static str,
    pub supports_request_bridge: bool,
    pub supports_sync_finalize_bridge: bool,
    pub supports_stream_bridge: bool,
    pub requires_eventstream_accept: bool,
    pub unwraps_response_envelope: bool,
}

const PROVIDER_ADAPTATION_SURFACES: &[ProviderAdaptationDescriptor] = &[
    ProviderAdaptationDescriptor {
        surface: ProviderAdaptationSurface::AntigravityGeminiChat,
        provider_type: Some(ANTIGRAVITY_PROVIDER_TYPE),
        envelope_name: ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
        anchor_api_format: "gemini:generate_content",
        supports_request_bridge: true,
        supports_sync_finalize_bridge: true,
        supports_stream_bridge: true,
        requires_eventstream_accept: false,
        unwraps_response_envelope: true,
    },
    ProviderAdaptationDescriptor {
        surface: ProviderAdaptationSurface::AntigravityGeminiCli,
        provider_type: Some(ANTIGRAVITY_PROVIDER_TYPE),
        envelope_name: ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
        anchor_api_format: "gemini:generate_content",
        supports_request_bridge: true,
        supports_sync_finalize_bridge: true,
        supports_stream_bridge: true,
        requires_eventstream_accept: false,
        unwraps_response_envelope: true,
    },
    ProviderAdaptationDescriptor {
        surface: ProviderAdaptationSurface::GeminiCliV1Internal,
        provider_type: None,
        envelope_name: GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME,
        anchor_api_format: "gemini:generate_content",
        supports_request_bridge: false,
        supports_sync_finalize_bridge: true,
        supports_stream_bridge: true,
        requires_eventstream_accept: false,
        unwraps_response_envelope: true,
    },
    ProviderAdaptationDescriptor {
        surface: ProviderAdaptationSurface::KiroClaudeCli,
        provider_type: Some(KIRO_PROVIDER_TYPE),
        envelope_name: KIRO_ENVELOPE_NAME,
        anchor_api_format: "claude:messages",
        supports_request_bridge: true,
        supports_sync_finalize_bridge: true,
        supports_stream_bridge: true,
        requires_eventstream_accept: true,
        unwraps_response_envelope: false,
    },
];

pub fn provider_adaptation_descriptor_for_envelope(
    envelope_name: &str,
    provider_api_format: &str,
) -> Option<&'static ProviderAdaptationDescriptor> {
    let envelope_name = envelope_name.trim();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    PROVIDER_ADAPTATION_SURFACES.iter().find(|descriptor| {
        descriptor.envelope_name.eq_ignore_ascii_case(envelope_name)
            && descriptor
                .anchor_api_format
                .eq_ignore_ascii_case(provider_api_format.as_str())
    })
}

pub fn provider_adaptation_descriptor_for_provider_type(
    provider_type: &str,
    provider_api_format: &str,
) -> Option<&'static ProviderAdaptationDescriptor> {
    let provider_type = provider_type.trim();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    PROVIDER_ADAPTATION_SURFACES.iter().find(|descriptor| {
        descriptor
            .provider_type
            .is_some_and(|value| value.eq_ignore_ascii_case(provider_type))
            && descriptor
                .anchor_api_format
                .eq_ignore_ascii_case(provider_api_format.as_str())
    })
}

pub fn provider_adaptation_anchor_api_format(
    envelope_name: &str,
    provider_api_format: &str,
) -> Option<&'static str> {
    provider_adaptation_descriptor_for_envelope(envelope_name, provider_api_format)
        .map(|descriptor| descriptor.anchor_api_format)
}

pub fn provider_adaptation_allows_sync_finalize_envelope(
    envelope_name: &str,
    provider_api_format: &str,
) -> bool {
    provider_adaptation_descriptor_for_envelope(envelope_name, provider_api_format)
        .is_some_and(|descriptor| descriptor.supports_sync_finalize_bridge)
}

pub fn provider_adaptation_requires_eventstream_accept(
    envelope_name: Option<&str>,
    provider_api_format: &str,
) -> bool {
    envelope_name
        .and_then(|value| provider_adaptation_descriptor_for_envelope(value, provider_api_format))
        .is_some_and(|descriptor| descriptor.requires_eventstream_accept)
}

pub fn provider_adaptation_should_unwrap_stream_envelope(
    envelope_name: &str,
    provider_api_format: &str,
) -> bool {
    provider_adaptation_descriptor_for_envelope(envelope_name, provider_api_format)
        .is_some_and(|descriptor| descriptor.unwraps_response_envelope)
}

#[cfg(test)]
mod tests {
    use super::{
        provider_adaptation_allows_sync_finalize_envelope, provider_adaptation_anchor_api_format,
        provider_adaptation_requires_eventstream_accept,
        provider_adaptation_should_unwrap_stream_envelope, ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
        GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME, KIRO_ENVELOPE_NAME,
    };

    #[test]
    fn resolves_private_surface_anchor_contracts() {
        assert_eq!(
            provider_adaptation_anchor_api_format(
                ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
                "gemini:generate_content"
            ),
            Some("gemini:generate_content")
        );
        assert_eq!(
            provider_adaptation_anchor_api_format(
                GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME,
                "gemini:generate_content"
            ),
            Some("gemini:generate_content")
        );
        assert_eq!(
            provider_adaptation_anchor_api_format(KIRO_ENVELOPE_NAME, "claude:messages"),
            Some("claude:messages")
        );
    }

    #[test]
    fn exposes_private_surface_capabilities() {
        assert!(provider_adaptation_allows_sync_finalize_envelope(
            ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
            "gemini:generate_content"
        ));
        assert!(provider_adaptation_should_unwrap_stream_envelope(
            GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME,
            "gemini:generate_content"
        ));
        assert!(provider_adaptation_requires_eventstream_accept(
            Some(KIRO_ENVELOPE_NAME),
            "claude:messages"
        ));
        assert!(!provider_adaptation_requires_eventstream_accept(
            Some(ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME),
            "gemini:generate_content"
        ));
    }
}
