pub mod canonical;
pub mod context;
pub mod conversion;
pub mod formats;
pub mod matrix;
pub mod planner;
pub mod proxy;
pub mod registry;
pub mod stream;

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
pub use context::{FormatContext, FormatError};
pub use formats::{
    is_openai_responses_compact_format, is_openai_responses_family_format,
    is_openai_responses_format, legacy_openai_format_alias_matches,
    normalize_legacy_openai_format_alias, openai_format_storage_aliases, FormatFamily, FormatId,
    FormatProfile,
};
pub use matrix::{
    request_candidate_api_format_preference, request_candidate_api_formats,
    request_conversion_kind, request_conversion_requires_enable_flag,
    sync_chat_response_conversion_kind, sync_cli_response_conversion_kind, RequestConversionKind,
    SyncChatResponseConversionKind, SyncCliResponseConversionKind,
};
pub use registry::{build_stream_transcoder, convert_request, convert_response};
