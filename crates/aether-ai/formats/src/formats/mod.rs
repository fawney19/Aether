pub mod aliyun;
pub mod claude;
pub mod context;
pub mod conversion;
pub mod doubao;
pub mod gemini;
pub mod id;
pub mod jina;
pub mod matrix;
pub mod openai;
pub mod registry;
pub mod shared;

pub use context::{FormatContext, FormatError};
pub use id::{
    api_format_alias_matches, api_format_defaults_to_client_error_failover,
    api_format_defaults_to_non_stream, api_format_permission_covers,
    api_format_permission_storage_aliases, api_format_storage_aliases,
    intersect_api_format_allowed_lists, is_openai_responses_compact_format,
    is_openai_responses_family_format, is_openai_responses_format, normalize_api_format_alias,
    FormatFamily, FormatId, FormatProfile,
};
