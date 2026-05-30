pub mod antigravity;
pub mod chatgpt_web;
pub mod codex;
pub mod default;
pub mod gemini_cli;
pub mod grok;
pub mod kiro;
pub mod unsupported;
pub mod windsurf;

pub use antigravity::AntigravityProviderPoolAdapter;
pub use antigravity::{
    build_antigravity_pool_quota_request, ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH,
};
pub use chatgpt_web::ChatGptWebProviderPoolAdapter;
pub use chatgpt_web::{
    build_chatgpt_web_pool_quota_request, enrich_chatgpt_web_quota_metadata,
    normalize_chatgpt_web_image_quota_limit, CHATGPT_WEB_CONVERSATION_INIT_PATH,
    CHATGPT_WEB_DEFAULT_BASE_URL,
};
pub use codex::CodexProviderPoolAdapter;
pub use codex::{build_codex_pool_quota_request, CODEX_WHAM_USAGE_URL};
pub use default::DefaultProviderPoolAdapter;
pub use gemini_cli::GeminiCliProviderPoolAdapter;
pub use gemini_cli::{
    build_gemini_cli_pool_quota_request, GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH, GEMINI_CLI_USER_AGENT,
};
pub use grok::{
    grok_mode_id_for_model, grok_pool_tier_from_quota_bucket, grok_quota_window_key_for_model,
    grok_supported_quota_windows_for_tier, GrokProviderPoolAdapter,
};
pub use kiro::KiroProviderPoolAdapter;
pub use kiro::{
    build_kiro_pool_quota_request, KiroPoolQuotaAuthInput, KIRO_USAGE_LIMITS_PATH,
    KIRO_USAGE_SDK_VERSION,
};
pub use unsupported::{
    UnsupportedQuotaProviderPoolAdapter, CLAUDE_CODE_PROVIDER_POOL_ADAPTER,
    VERTEX_AI_PROVIDER_POOL_ADAPTER,
};
pub use windsurf::{
    build_windsurf_pool_model_configs_request,
    build_windsurf_pool_model_configs_request_with_base_url, build_windsurf_pool_quota_request,
    build_windsurf_pool_quota_request_with_base_url, build_windsurf_pool_rate_limit_request,
    build_windsurf_pool_rate_limit_request_with_base_url, WindsurfProviderPoolAdapter,
    WINDSURF_DEFAULT_BASE_URL, WINDSURF_MODEL_CONFIGS_PATH, WINDSURF_RATE_LIMIT_PATH,
    WINDSURF_USER_STATUS_PATH,
};
