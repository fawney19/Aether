pub mod canonical;
mod error;
mod registry;
pub mod request;
pub mod response;

pub use aether_ai_formats::{
    convert_request, convert_response, FormatContext, FormatError, FormatFamily, FormatId,
    FormatProfile,
};
pub use canonical::{
    canonical_request_unknown_block_count, canonical_response_unknown_block_count,
    canonical_to_claude_request, canonical_to_claude_response, canonical_to_gemini_request,
    canonical_to_gemini_response, canonical_to_openai_chat_request,
    canonical_to_openai_chat_response, canonical_to_openai_responses_compact_request,
    canonical_to_openai_responses_compact_response, canonical_to_openai_responses_request,
    canonical_to_openai_responses_response, canonical_unknown_block_count,
    from_claude_to_canonical_request, from_claude_to_canonical_response,
    from_gemini_to_canonical_request, from_gemini_to_canonical_response,
    from_openai_chat_to_canonical_request, from_openai_chat_to_canonical_response,
    from_openai_responses_to_canonical_request, from_openai_responses_to_canonical_response,
    CanonicalContentBlock, CanonicalGenerationConfig, CanonicalInstruction, CanonicalMessage,
    CanonicalRequest, CanonicalResponse, CanonicalResponseFormat, CanonicalResponseOutput,
    CanonicalRole, CanonicalStopReason, CanonicalStreamEvent, CanonicalStreamFrame,
    CanonicalThinkingConfig, CanonicalToolChoice, CanonicalToolDefinition, CanonicalUsage,
};
pub use error::{
    build_core_error_body_for_client_format, core_error_background_report_kind,
    core_error_default_client_api_format, core_success_background_report_kind,
    is_core_error_finalize_kind, LocalCoreSyncErrorKind,
};
pub use registry::{
    request_candidate_api_format_preference, request_candidate_api_formats,
    request_conversion_direct_auth, request_conversion_enabled_for_transport,
    request_conversion_kind, request_conversion_requires_enable_flag,
    request_conversion_transport_supported, request_conversion_transport_unsupported_reason,
    request_pair_allowed_for_transport, sync_chat_response_conversion_kind,
    sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
    SyncCliResponseConversionKind,
};
