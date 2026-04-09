//! Standard finalize surface for standard contract sync/stream compilation.

#[path = "stream_core/mod.rs"]
mod stream;

pub(crate) use crate::ai_pipeline::{
    aggregate_standard_chat_stream_sync_response, aggregate_standard_cli_stream_sync_response,
    build_openai_cli_response, convert_claude_chat_response_to_openai_chat,
    convert_claude_cli_response_to_openai_cli, convert_gemini_chat_response_to_openai_chat,
    convert_gemini_cli_response_to_openai_cli, convert_openai_chat_response_to_claude_chat,
    convert_openai_chat_response_to_gemini_chat, convert_openai_chat_response_to_openai_cli,
    convert_openai_cli_response_to_openai_chat, convert_standard_chat_response,
    convert_standard_cli_response,
    maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload,
    maybe_build_openai_cli_cross_format_sync_product_from_normalized_payload,
    maybe_build_openai_cli_same_family_sync_body_from_normalized_payload,
    maybe_build_standard_cross_format_sync_product,
    maybe_build_standard_cross_format_sync_product_from_normalized_payload,
    maybe_build_standard_same_format_sync_body_from_normalized_payload,
    maybe_build_standard_sync_finalize_product_from_normalized_payload,
    StandardCrossFormatSyncProduct, StandardSyncFinalizeNormalizedProduct,
};
pub(crate) use stream::*;
