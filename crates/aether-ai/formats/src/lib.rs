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
    api_format_alias_matches, api_format_defaults_to_client_error_failover,
    api_format_defaults_to_non_stream, api_format_permission_covers,
    api_format_permission_storage_aliases, api_format_storage_aliases,
    api_format_uses_body_stream_field, intersect_api_format_allowed_lists,
    is_openai_responses_compact_format, is_openai_responses_family_format,
    is_openai_responses_format, normalize_api_format_alias, FormatFamily, FormatId, FormatProfile,
};
pub use formats::matrix::{
    is_embedding_api_format, is_gemini_interactions_api_format, is_rerank_api_format,
    request_candidate_api_format_preference, request_candidate_api_formats,
    request_conversion_kind, request_conversion_requires_enable_flag,
    sync_chat_response_conversion_kind, sync_cli_response_conversion_kind, RequestConversionKind,
    SyncChatResponseConversionKind, SyncCliResponseConversionKind,
};
pub use formats::openai::prompt_cache::resolve_openai_prompt_cache_ttl_minutes;
pub use formats::openai::prompt_cache::{
    validate_openai_prompt_cache_request, OpenAiPromptCacheContractViolation,
    OpenAiPromptCacheViolationKind,
};
pub use formats::openai::reasoning::{
    validate_openai_reasoning_request, OpenAiReasoningContractViolation,
    OpenAiReasoningViolationKind,
};
pub use formats::openai::request_contract::{
    finalize_openai_provider_request,
    finalize_openai_provider_request_with_codex_model_capabilities,
    validate_openai_provider_request_contract, OpenAiProviderRequestContractViolation,
    OpenAiProviderRequestFinalization,
};
pub use formats::openai::responses::codex::{
    build_codex_model_catalog_metadata, bundled_codex_model_cards, effective_codex_model_cards,
    parse_codex_auth_identity, resolve_codex_responses_model_capabilities, CodexAuthIdentity,
    CodexResponsesModelCapabilities, CODEX_CLIENT_ORIGINATOR, CODEX_CLIENT_USER_AGENT,
    CODEX_CLIENT_VERSION, CODEX_MODEL_CATALOG_METADATA_FIELD, CODEX_RESPONSES_LITE_HEADER,
};
pub use formats::openai::responses::request::{
    validate_openai_responses_request_contract, OpenAiResponsesRequestContractViolation,
};
pub use formats::openai::responses::{
    openai_responses_request_operation, OPENAI_RESPONSES_OPERATION_COMPACT,
};
pub use formats::registry::{
    build_stream_transcoder, convert_request, convert_request_pure,
    convert_request_pure_with_context, convert_response, convert_response_pure, emit_request_pure,
    emit_response_pure, parse_request_pure, parse_response_pure,
};
pub use formats::shared::model_directives::{
    apply_model_directive_mapping_patch, apply_model_directive_overrides_from_model,
    apply_model_directive_overrides_from_request, claude_model_uses_adaptive_effort,
    default_model_directive_mapping_patch, default_model_directive_suffixes,
    default_model_directives_config, extract_gemini_model_from_path,
    gemini_model_uses_thinking_level, model_directive_base_model,
    model_directive_builtin_suffix_supported_for_source_model,
    model_directive_suffix_has_builtin_mapping, normalize_model_directive_model,
    openai_model_supports_prompt_cache_options, parse_model_directive,
    parse_model_directive_with_suffixes, reasoning_effort_supported_for_model, ModelDirective,
    ModelDirectiveSuffixResolution, ModelOverride, ReasoningEffort, ServiceTier,
    CROSS_PROVIDER_MODEL_DIRECTIVE_SUFFIXES, MODEL_DIRECTIVE_API_FORMATS,
    OPENAI_MODEL_DIRECTIVE_SUFFIXES,
};
pub use formats::shared::request::{
    endpoint_config_forces_upstream_stream_policy, enforce_request_body_stream_field,
    forbid_upstream_streaming_for_provider, force_upstream_streaming_for_provider,
    parse_direct_request_body, resolve_upstream_is_stream_for_provider,
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
