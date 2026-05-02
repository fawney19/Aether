pub(crate) mod private_envelope;

pub(crate) mod kiro {
    pub(crate) use crate::ai_serving::pure::KiroToClaudeCliStreamState;
}

pub(crate) use crate::ai_serving::{
    provider_adaptation_allows_sync_finalize_envelope, provider_adaptation_anchor_api_format,
    provider_adaptation_descriptor_for_envelope, provider_adaptation_descriptor_for_provider_type,
    provider_adaptation_requires_eventstream_accept,
    provider_adaptation_should_unwrap_stream_envelope, ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
    GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME,
};
pub(crate) use kiro::KiroToClaudeCliStreamState;
pub(crate) use private_envelope::{
    maybe_build_provider_private_stream_normalizer,
    maybe_normalize_provider_private_sync_report_payload,
    normalize_provider_private_report_context, normalize_provider_private_response_value,
    provider_private_response_allows_sync_finalize, transform_provider_private_stream_line,
    ProviderPrivateStreamNormalizer,
};
