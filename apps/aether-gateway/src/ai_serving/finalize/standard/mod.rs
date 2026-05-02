//! Standard finalize surface for standard contract sync/stream compilation.

pub(crate) use crate::ai_serving::{
    aggregate_standard_chat_stream_sync_response, aggregate_standard_cli_stream_sync_response,
    build_openai_responses_response, convert_claude_chat_response_to_openai_chat,
    convert_claude_response_to_openai_responses, convert_gemini_chat_response_to_openai_chat,
    convert_gemini_response_to_openai_responses, convert_openai_chat_response_to_claude_chat,
    convert_openai_chat_response_to_gemini_chat, convert_openai_chat_response_to_openai_responses,
    convert_openai_responses_response_to_openai_chat, convert_standard_chat_response,
    convert_standard_cli_response,
    maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload,
    maybe_build_openai_responses_cross_format_sync_product_from_normalized_payload,
    maybe_build_openai_responses_same_family_sync_body_from_normalized_payload,
    maybe_build_standard_cross_format_sync_product,
    maybe_build_standard_cross_format_sync_product_from_normalized_payload,
    maybe_build_standard_same_format_sync_body_from_normalized_payload,
    maybe_build_standard_sync_finalize_product_from_normalized_payload,
    StandardCrossFormatSyncProduct, StandardSyncFinalizeNormalizedProduct,
};
