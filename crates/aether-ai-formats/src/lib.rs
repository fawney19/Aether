extern crate self as aether_ai_formats;

pub mod api;
pub mod contracts;
pub mod formats;
pub mod protocol;
pub mod provider_compat;

pub use formats::context::{
    ConversionFieldRecord, ConversionFieldStatus, ConversionReport, Converted, FormatContext,
    FormatError,
};
pub use formats::id::{
    api_format_alias_matches, api_format_storage_aliases, api_format_uses_body_stream_field,
    is_openai_responses_compact_format, is_openai_responses_family_format,
    is_openai_responses_format, normalize_api_format_alias, FormatFamily, FormatId, FormatProfile,
};
pub use formats::matrix::{
    is_embedding_api_format, is_rerank_api_format, request_candidate_api_format_preference,
    request_candidate_api_formats, request_conversion_kind,
    request_conversion_requires_enable_flag, sync_chat_response_conversion_kind,
    sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
    SyncCliResponseConversionKind,
};
pub use formats::registry::{
    build_stream_transcoder, convert_request, convert_request_pure,
    convert_request_pure_with_context, convert_response, convert_response_pure, emit_request_pure,
    emit_response_pure, parse_request_pure, parse_response_pure,
};
pub use formats::shared::model_directives::{
    apply_model_directive_mapping_patch, apply_model_directive_overrides_from_model,
    apply_model_directive_overrides_from_request, claude_model_uses_adaptive_effort,
    extract_gemini_model_from_path, gemini_model_uses_thinking_level, model_directive_base_model,
    normalize_model_directive_model, parse_model_directive, ModelDirective, ModelOverride,
    ReasoningEffort, ServiceTier,
};
pub use formats::shared::request::{
    endpoint_config_forces_upstream_stream_policy, enforce_request_body_stream_field,
    resolve_upstream_is_stream_from_endpoint_config, UPSTREAM_IS_STREAM_KEY,
};
pub use protocol::canonical::{
    canonical_request_unknown_block_count, canonical_response_unknown_block_count,
    canonical_to_claude_request, canonical_to_claude_response, canonical_to_embedding_response,
    canonical_to_gemini_request, canonical_to_gemini_response, canonical_to_openai_chat_request,
    canonical_to_openai_chat_response, canonical_to_openai_responses_compact_request,
    canonical_to_openai_responses_compact_response, canonical_to_openai_responses_request,
    canonical_to_openai_responses_response, canonical_unknown_block_count,
    from_claude_to_canonical_request, from_claude_to_canonical_response,
    from_embedding_to_canonical_response, from_gemini_to_canonical_request,
    from_gemini_to_canonical_response, from_openai_chat_to_canonical_request,
    from_openai_chat_to_canonical_response, from_openai_responses_to_canonical_request,
    from_openai_responses_to_canonical_response, CanonicalContentBlock, CanonicalEmbedding,
    CanonicalEmbeddingContent, CanonicalEmbeddingInput, CanonicalEmbeddingRequest,
    CanonicalEmbeddingResponse, CanonicalGenerationConfig, CanonicalInstruction, CanonicalMessage,
    CanonicalRequest, CanonicalResponse, CanonicalResponseFormat, CanonicalResponseOutput,
    CanonicalRole, CanonicalStopReason, CanonicalStreamEvent, CanonicalStreamFrame,
    CanonicalThinkingConfig, CanonicalToolChoice, CanonicalToolDefinition, CanonicalUsage,
};
