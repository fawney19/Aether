pub mod claude;
pub mod codex;
pub mod family;
pub mod gemini;
pub mod matrix;
pub mod normalize;
pub mod openai_cli;

pub use codex::{
    apply_codex_openai_cli_special_body_edits, apply_codex_openai_cli_special_headers,
    apply_openai_compact_special_body_edits,
};
pub use family::{LocalStandardSourceFamily, LocalStandardSourceMode, LocalStandardSpec};
pub use matrix::{
    build_standard_request_body, build_standard_upstream_url,
    normalize_standard_request_to_openai_chat_request,
};
pub use normalize::{
    build_cross_format_openai_chat_request_body, build_cross_format_openai_cli_request_body,
    build_local_openai_chat_request_body, build_local_openai_cli_request_body,
};
