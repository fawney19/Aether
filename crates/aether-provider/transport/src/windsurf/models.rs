#[derive(Debug, Clone, PartialEq)]
pub struct WindsurfModel {
    pub canonical_name: &'static str,
    pub enum_value: u32,
    pub model_uid: Option<&'static str>,
    pub credit_multiplier: f32,
    pub provider: &'static str,
    pub deprecated: bool,
}

#[rustfmt::skip]
const MODELS: &[WindsurfModel] = &[
    WindsurfModel { canonical_name: "claude-3.5-sonnet", enum_value: 166, model_uid: None, credit_multiplier: 2.0, provider: "anthropic", deprecated: true },
    WindsurfModel { canonical_name: "claude-3.7-sonnet", enum_value: 226, model_uid: None, credit_multiplier: 2.0, provider: "anthropic", deprecated: true },
    WindsurfModel { canonical_name: "claude-3.7-sonnet-thinking", enum_value: 227, model_uid: None, credit_multiplier: 3.0, provider: "anthropic", deprecated: true },
    WindsurfModel { canonical_name: "claude-4-sonnet", enum_value: 281, model_uid: Some("MODEL_CLAUDE_4_SONNET"), credit_multiplier: 2.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4-sonnet-thinking", enum_value: 282, model_uid: Some("MODEL_CLAUDE_4_SONNET_THINKING"), credit_multiplier: 3.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4-opus", enum_value: 290, model_uid: Some("MODEL_CLAUDE_4_OPUS"), credit_multiplier: 4.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4-opus-thinking", enum_value: 291, model_uid: Some("MODEL_CLAUDE_4_OPUS_THINKING"), credit_multiplier: 5.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4.1-opus", enum_value: 328, model_uid: Some("MODEL_CLAUDE_4_1_OPUS"), credit_multiplier: 4.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4.1-opus-thinking", enum_value: 329, model_uid: Some("MODEL_CLAUDE_4_1_OPUS_THINKING"), credit_multiplier: 5.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4.5-haiku", enum_value: 0, model_uid: Some("MODEL_PRIVATE_11"), credit_multiplier: 1.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4.5-sonnet", enum_value: 353, model_uid: Some("MODEL_PRIVATE_2"), credit_multiplier: 2.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4.5-sonnet-thinking", enum_value: 354, model_uid: Some("MODEL_PRIVATE_3"), credit_multiplier: 3.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4.5-opus", enum_value: 391, model_uid: Some("MODEL_CLAUDE_4_5_OPUS"), credit_multiplier: 4.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-4.5-opus-thinking", enum_value: 392, model_uid: Some("MODEL_CLAUDE_4_5_OPUS_THINKING"), credit_multiplier: 5.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-sonnet-4.6", enum_value: 0, model_uid: Some("claude-sonnet-4-6"), credit_multiplier: 4.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-sonnet-4.6-thinking", enum_value: 0, model_uid: Some("claude-sonnet-4-6-thinking"), credit_multiplier: 6.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-sonnet-4.6-1m", enum_value: 0, model_uid: Some("claude-sonnet-4-6-1m"), credit_multiplier: 12.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-sonnet-4.6-thinking-1m", enum_value: 0, model_uid: Some("claude-sonnet-4-6-thinking-1m"), credit_multiplier: 16.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4.6", enum_value: 0, model_uid: Some("claude-opus-4-6"), credit_multiplier: 6.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4.6-thinking", enum_value: 0, model_uid: Some("claude-opus-4-6-thinking"), credit_multiplier: 8.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-medium", enum_value: 0, model_uid: Some("claude-opus-4-7-medium"), credit_multiplier: 8.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-low", enum_value: 0, model_uid: Some("claude-opus-4-7-low"), credit_multiplier: 6.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-high", enum_value: 0, model_uid: Some("claude-opus-4-7-high"), credit_multiplier: 10.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-xhigh", enum_value: 0, model_uid: Some("claude-opus-4-7-xhigh"), credit_multiplier: 12.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-medium-thinking", enum_value: 0, model_uid: Some("claude-opus-4-7-medium-thinking"), credit_multiplier: 10.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-high-thinking", enum_value: 0, model_uid: Some("claude-opus-4-7-high-thinking"), credit_multiplier: 12.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-xhigh-thinking", enum_value: 0, model_uid: Some("claude-opus-4-7-xhigh-thinking"), credit_multiplier: 16.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "claude-opus-4-7-max", enum_value: 0, model_uid: Some("claude-opus-4-7-max"), credit_multiplier: 16.0, provider: "anthropic", deprecated: false },
    WindsurfModel { canonical_name: "gpt-4o", enum_value: 109, model_uid: Some("MODEL_CHAT_GPT_4O_2024_08_06"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-4o-mini", enum_value: 113, model_uid: None, credit_multiplier: 0.5, provider: "openai", deprecated: true },
    WindsurfModel { canonical_name: "gpt-4.1", enum_value: 259, model_uid: Some("MODEL_CHAT_GPT_4_1_2025_04_14"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-4.1-mini", enum_value: 260, model_uid: None, credit_multiplier: 0.5, provider: "openai", deprecated: true },
    WindsurfModel { canonical_name: "gpt-4.1-nano", enum_value: 261, model_uid: None, credit_multiplier: 0.25, provider: "openai", deprecated: true },
    WindsurfModel { canonical_name: "gpt-5", enum_value: 340, model_uid: Some("MODEL_PRIVATE_6"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5-medium", enum_value: 0, model_uid: Some("MODEL_PRIVATE_7"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5-high", enum_value: 0, model_uid: Some("MODEL_PRIVATE_8"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5-mini", enum_value: 337, model_uid: None, credit_multiplier: 0.25, provider: "openai", deprecated: true },
    WindsurfModel { canonical_name: "gpt-5-codex", enum_value: 346, model_uid: Some("MODEL_CHAT_GPT_5_CODEX"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1", enum_value: 0, model_uid: Some("MODEL_PRIVATE_12"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-low", enum_value: 0, model_uid: Some("MODEL_PRIVATE_13"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-medium", enum_value: 0, model_uid: Some("MODEL_PRIVATE_14"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-high", enum_value: 0, model_uid: Some("MODEL_PRIVATE_15"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-fast", enum_value: 0, model_uid: Some("MODEL_PRIVATE_20"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-low-fast", enum_value: 0, model_uid: Some("MODEL_PRIVATE_21"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-medium-fast", enum_value: 0, model_uid: Some("MODEL_PRIVATE_22"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-high-fast", enum_value: 0, model_uid: Some("MODEL_PRIVATE_23"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-codex-low", enum_value: 0, model_uid: Some("MODEL_GPT_5_1_CODEX_LOW"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-codex-medium", enum_value: 0, model_uid: Some("MODEL_PRIVATE_9"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-codex-mini-low", enum_value: 0, model_uid: Some("MODEL_GPT_5_1_CODEX_MINI_LOW"), credit_multiplier: 0.25, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-codex-mini", enum_value: 0, model_uid: Some("MODEL_PRIVATE_19"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-codex-max-low", enum_value: 0, model_uid: Some("MODEL_GPT_5_1_CODEX_MAX_LOW"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-codex-max-medium", enum_value: 0, model_uid: Some("MODEL_GPT_5_1_CODEX_MAX_MEDIUM"), credit_multiplier: 1.25, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.1-codex-max-high", enum_value: 0, model_uid: Some("MODEL_GPT_5_1_CODEX_MAX_HIGH"), credit_multiplier: 1.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2", enum_value: 401, model_uid: Some("MODEL_GPT_5_2_MEDIUM"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-none", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_NONE"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-low", enum_value: 400, model_uid: Some("MODEL_GPT_5_2_LOW"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-high", enum_value: 402, model_uid: Some("MODEL_GPT_5_2_HIGH"), credit_multiplier: 3.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-xhigh", enum_value: 403, model_uid: Some("MODEL_GPT_5_2_XHIGH"), credit_multiplier: 8.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-none-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_NONE_PRIORITY"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-low-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_LOW_PRIORITY"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-medium-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_MEDIUM_PRIORITY"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-high-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_HIGH_PRIORITY"), credit_multiplier: 6.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-xhigh-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_XHIGH_PRIORITY"), credit_multiplier: 16.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-low", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_LOW"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-medium", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_MEDIUM"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-high", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_HIGH"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-xhigh", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_XHIGH"), credit_multiplier: 3.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-low-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_LOW_PRIORITY"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-medium-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_MEDIUM_PRIORITY"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-high-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_HIGH_PRIORITY"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.2-codex-xhigh-fast", enum_value: 0, model_uid: Some("MODEL_GPT_5_2_CODEX_XHIGH_PRIORITY"), credit_multiplier: 6.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex", enum_value: 0, model_uid: Some("gpt-5-3-codex-medium"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-none", enum_value: 0, model_uid: Some("gpt-5-4-none"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-low", enum_value: 0, model_uid: Some("gpt-5-4-low"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-medium", enum_value: 0, model_uid: Some("gpt-5-4-medium"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-high", enum_value: 0, model_uid: Some("gpt-5-4-high"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-xhigh", enum_value: 0, model_uid: Some("gpt-5-4-xhigh"), credit_multiplier: 8.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-mini-low", enum_value: 0, model_uid: Some("gpt-5-4-mini-low"), credit_multiplier: 1.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-mini-medium", enum_value: 0, model_uid: Some("gpt-5-4-mini-medium"), credit_multiplier: 1.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-mini-high", enum_value: 0, model_uid: Some("gpt-5-4-mini-high"), credit_multiplier: 4.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.4-mini-xhigh", enum_value: 0, model_uid: Some("gpt-5-4-mini-xhigh"), credit_multiplier: 12.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5", enum_value: 0, model_uid: Some("gpt-5-5-medium"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-none", enum_value: 0, model_uid: Some("gpt-5-5-none"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-low", enum_value: 0, model_uid: Some("gpt-5-5-low"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-medium", enum_value: 0, model_uid: Some("gpt-5-5-medium"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-high", enum_value: 0, model_uid: Some("gpt-5-5-high"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-xhigh", enum_value: 0, model_uid: Some("gpt-5-5-xhigh"), credit_multiplier: 8.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-none-fast", enum_value: 0, model_uid: Some("gpt-5-5-none-priority"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-low-fast", enum_value: 0, model_uid: Some("gpt-5-5-low-priority"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-medium-fast", enum_value: 0, model_uid: Some("gpt-5-5-medium-priority"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-high-fast", enum_value: 0, model_uid: Some("gpt-5-5-high-priority"), credit_multiplier: 8.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.5-xhigh-fast", enum_value: 0, model_uid: Some("gpt-5-5-xhigh-priority"), credit_multiplier: 16.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex-low", enum_value: 0, model_uid: Some("gpt-5-3-codex-low"), credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex-high", enum_value: 0, model_uid: Some("gpt-5-3-codex-high"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex-xhigh", enum_value: 0, model_uid: Some("gpt-5-3-codex-xhigh"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex-low-fast", enum_value: 0, model_uid: Some("gpt-5-3-codex-low-priority"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex-medium-fast", enum_value: 0, model_uid: Some("gpt-5-3-codex-medium-priority"), credit_multiplier: 2.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex-high-fast", enum_value: 0, model_uid: Some("gpt-5-3-codex-high-priority"), credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-5.3-codex-xhigh-fast", enum_value: 0, model_uid: Some("gpt-5-3-codex-xhigh-priority"), credit_multiplier: 6.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gpt-oss-120b", enum_value: 0, model_uid: Some("MODEL_GPT_OSS_120B"), credit_multiplier: 0.25, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "o3-mini", enum_value: 207, model_uid: None, credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "o3", enum_value: 218, model_uid: Some("MODEL_CHAT_O3"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "o3-high", enum_value: 0, model_uid: Some("MODEL_CHAT_O3_HIGH"), credit_multiplier: 1.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "o3-pro", enum_value: 294, model_uid: None, credit_multiplier: 4.0, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "o4-mini", enum_value: 264, model_uid: None, credit_multiplier: 0.5, provider: "openai", deprecated: false },
    WindsurfModel { canonical_name: "gemini-2.5-pro", enum_value: 246, model_uid: Some("MODEL_GOOGLE_GEMINI_2_5_PRO"), credit_multiplier: 1.0, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-2.5-flash", enum_value: 312, model_uid: Some("MODEL_GOOGLE_GEMINI_2_5_FLASH"), credit_multiplier: 0.5, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-3.0-pro", enum_value: 412, model_uid: Some("MODEL_GOOGLE_GEMINI_3_0_PRO_LOW"), credit_multiplier: 1.0, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-3.0-flash-minimal", enum_value: 0, model_uid: Some("MODEL_GOOGLE_GEMINI_3_0_FLASH_MINIMAL"), credit_multiplier: 0.75, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-3.0-flash-low", enum_value: 0, model_uid: Some("MODEL_GOOGLE_GEMINI_3_0_FLASH_LOW"), credit_multiplier: 1.0, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-3.0-flash", enum_value: 415, model_uid: Some("MODEL_GOOGLE_GEMINI_3_0_FLASH_MEDIUM"), credit_multiplier: 1.0, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-3.0-flash-high", enum_value: 0, model_uid: Some("MODEL_GOOGLE_GEMINI_3_0_FLASH_HIGH"), credit_multiplier: 1.75, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-3.1-pro-low", enum_value: 0, model_uid: Some("gemini-3-1-pro-low"), credit_multiplier: 1.0, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "gemini-3.1-pro-high", enum_value: 0, model_uid: Some("gemini-3-1-pro-high"), credit_multiplier: 2.0, provider: "google", deprecated: false },
    WindsurfModel { canonical_name: "deepseek-v3", enum_value: 205, model_uid: None, credit_multiplier: 0.5, provider: "deepseek", deprecated: true },
    WindsurfModel { canonical_name: "deepseek-v3-2", enum_value: 409, model_uid: None, credit_multiplier: 0.5, provider: "deepseek", deprecated: true },
    WindsurfModel { canonical_name: "deepseek-r1", enum_value: 206, model_uid: None, credit_multiplier: 1.0, provider: "deepseek", deprecated: true },
    WindsurfModel { canonical_name: "grok-3", enum_value: 217, model_uid: Some("MODEL_XAI_GROK_3"), credit_multiplier: 1.0, provider: "xai", deprecated: false },
    WindsurfModel { canonical_name: "grok-3-mini", enum_value: 234, model_uid: None, credit_multiplier: 0.5, provider: "xai", deprecated: true },
    WindsurfModel { canonical_name: "grok-3-mini-thinking", enum_value: 0, model_uid: Some("MODEL_XAI_GROK_3_MINI_REASONING"), credit_multiplier: 0.125, provider: "xai", deprecated: false },
    WindsurfModel { canonical_name: "grok-code-fast-1", enum_value: 0, model_uid: Some("MODEL_PRIVATE_4"), credit_multiplier: 0.5, provider: "xai", deprecated: false },
    WindsurfModel { canonical_name: "qwen-3", enum_value: 324, model_uid: None, credit_multiplier: 0.5, provider: "alibaba", deprecated: true },
    WindsurfModel { canonical_name: "kimi-k2", enum_value: 323, model_uid: Some("MODEL_KIMI_K2"), credit_multiplier: 0.5, provider: "moonshot", deprecated: false },
    WindsurfModel { canonical_name: "kimi-k2-thinking", enum_value: 394, model_uid: Some("MODEL_KIMI_K2_THINKING"), credit_multiplier: 1.0, provider: "moonshot", deprecated: false },
    WindsurfModel { canonical_name: "kimi-k2.5", enum_value: 0, model_uid: Some("kimi-k2-5"), credit_multiplier: 1.0, provider: "moonshot", deprecated: false },
    WindsurfModel { canonical_name: "kimi-k2-6", enum_value: 0, model_uid: Some("kimi-k2-6"), credit_multiplier: 1.0, provider: "moonshot", deprecated: false },
    WindsurfModel { canonical_name: "glm-4.7", enum_value: 417, model_uid: Some("MODEL_GLM_4_7"), credit_multiplier: 0.25, provider: "zhipu", deprecated: false },
    WindsurfModel { canonical_name: "glm-4.7-fast", enum_value: 418, model_uid: Some("MODEL_GLM_4_7_FAST"), credit_multiplier: 0.5, provider: "zhipu", deprecated: false },
    WindsurfModel { canonical_name: "glm-5", enum_value: 0, model_uid: Some("glm-5"), credit_multiplier: 1.5, provider: "zhipu", deprecated: false },
    WindsurfModel { canonical_name: "glm-5.1", enum_value: 0, model_uid: Some("glm-5-1"), credit_multiplier: 1.5, provider: "zhipu", deprecated: false },
    WindsurfModel { canonical_name: "minimax-m2.5", enum_value: 419, model_uid: Some("MODEL_MINIMAX_M2_1"), credit_multiplier: 1.0, provider: "minimax", deprecated: false },
    WindsurfModel { canonical_name: "swe-1.5", enum_value: 377, model_uid: Some("MODEL_SWE_1_5_SLOW"), credit_multiplier: 0.5, provider: "windsurf", deprecated: false },
    WindsurfModel { canonical_name: "swe-1.5-fast", enum_value: 359, model_uid: Some("MODEL_SWE_1_5"), credit_multiplier: 0.5, provider: "windsurf", deprecated: false },
    WindsurfModel { canonical_name: "swe-1.5-thinking", enum_value: 369, model_uid: Some("MODEL_SWE_1_5_THINKING"), credit_multiplier: 0.75, provider: "windsurf", deprecated: false },
    WindsurfModel { canonical_name: "swe-1.6", enum_value: 420, model_uid: Some("MODEL_SWE_1_6"), credit_multiplier: 0.5, provider: "windsurf", deprecated: false },
    WindsurfModel { canonical_name: "swe-1.6-fast", enum_value: 421, model_uid: Some("MODEL_SWE_1_6_FAST"), credit_multiplier: 0.5, provider: "windsurf", deprecated: false },
    WindsurfModel { canonical_name: "adaptive", enum_value: 0, model_uid: Some("adaptive"), credit_multiplier: 1.0, provider: "windsurf", deprecated: true },
    WindsurfModel { canonical_name: "arena-fast", enum_value: 0, model_uid: Some("arena-fast"), credit_multiplier: 0.5, provider: "windsurf", deprecated: true },
    WindsurfModel { canonical_name: "arena-smart", enum_value: 0, model_uid: Some("arena-smart"), credit_multiplier: 1.0, provider: "windsurf", deprecated: true },
];

#[rustfmt::skip]
const ALIASES: &[(&str, &str)] = &[
    ("claude-3-5-haiku-20241022", "claude-4.5-haiku"),
    ("claude-3-5-haiku-latest", "claude-4.5-haiku"),
    ("claude-3-5-sonnet-20240620", "claude-3.5-sonnet"),
    ("claude-3-5-sonnet-20241022", "claude-3.5-sonnet"),
    ("claude-3-5-sonnet-latest", "claude-3.5-sonnet"),
    ("claude-3-7-sonnet-20250219", "claude-3.7-sonnet"),
    ("claude-3-7-sonnet-latest", "claude-3.7-sonnet"),
    ("claude-4.6", "claude-sonnet-4.6"),
    ("claude-4.6-1m", "claude-sonnet-4.6-1m"),
    ("claude-4.6-thinking", "claude-sonnet-4.6-thinking"),
    ("claude-4.6-thinking-1m", "claude-sonnet-4.6-thinking-1m"),
    ("claude-haiku-3-5", "claude-4.5-haiku"),
    ("claude-haiku-3-5-latest", "claude-4.5-haiku"),
    ("claude-haiku-4-5", "claude-4.5-haiku"),
    ("claude-haiku-4-5-20251001", "claude-4.5-haiku"),
    ("claude-haiku-4-5-latest", "claude-4.5-haiku"),
    ("claude-haiku-4.5", "claude-4.5-haiku"),
    ("claude-haiku-4.5-latest", "claude-4.5-haiku"),
    ("claude-opus-4-0", "claude-4-opus"),
    ("claude-opus-4-1", "claude-4.1-opus"),
    ("claude-opus-4-1-20250805", "claude-4.1-opus"),
    ("claude-opus-4-20250514", "claude-4-opus"),
    ("claude-opus-4-5", "claude-4.5-opus"),
    ("claude-opus-4-5-20251101", "claude-4.5-opus"),
    ("claude-opus-4-5-latest", "claude-4.5-opus"),
    ("claude-opus-4-6", "claude-opus-4.6"),
    ("claude-opus-4-6-thinking", "claude-opus-4.6-thinking"),
    ("claude-opus-4-7", "claude-opus-4-7-medium"),
    ("claude-opus-4-7-latest", "claude-opus-4-7-medium"),
    ("claude-opus-4-7-thinking", "claude-opus-4-7-medium-thinking"),
    ("claude-opus-4.5", "claude-4.5-opus"),
    ("claude-opus-4.5-thinking", "claude-4.5-opus-thinking"),
    ("claude-opus-4.7", "claude-opus-4-7-medium"),
    ("claude-opus-4.7-high", "claude-opus-4-7-high"),
    ("claude-opus-4.7-high-thinking", "claude-opus-4-7-high-thinking"),
    ("claude-opus-4.7-low", "claude-opus-4-7-low"),
    ("claude-opus-4.7-max", "claude-opus-4-7-max"),
    ("claude-opus-4.7-medium", "claude-opus-4-7-medium"),
    ("claude-opus-4.7-medium-thinking", "claude-opus-4-7-medium-thinking"),
    ("claude-opus-4.7-thinking", "claude-opus-4-7-medium-thinking"),
    ("claude-opus-4.7-xhigh", "claude-opus-4-7-xhigh"),
    ("claude-opus-4.7-xhigh-thinking", "claude-opus-4-7-xhigh-thinking"),
    ("claude-sonnet-4-0", "claude-4-sonnet"),
    ("claude-sonnet-4-20250514", "claude-4-sonnet"),
    ("claude-sonnet-4-5", "claude-4.5-sonnet"),
    ("claude-sonnet-4-5-20250929", "claude-4.5-sonnet"),
    ("claude-sonnet-4-5-latest", "claude-4.5-sonnet"),
    ("claude-sonnet-4-6", "claude-sonnet-4.6"),
    ("claude-sonnet-4-6-1m", "claude-sonnet-4.6-1m"),
    ("claude-sonnet-4-6-thinking", "claude-sonnet-4.6-thinking"),
    ("claude-sonnet-4-6-thinking-1m", "claude-sonnet-4.6-thinking-1m"),
    ("claude-sonnet-4.5", "claude-4.5-sonnet"),
    ("claude-sonnet-4.5-thinking", "claude-4.5-sonnet-thinking"),
    ("gpt-4.1-2025-04-14", "gpt-4.1"),
    ("gpt-4.1-mini-2025-04-14", "gpt-4.1-mini"),
    ("gpt-4.1-nano-2025-04-14", "gpt-4.1-nano"),
    ("gpt-4o-2024-05-13", "gpt-4o"),
    ("gpt-4o-2024-08-06", "gpt-4o"),
    ("gpt-4o-2024-11-20", "gpt-4o"),
    ("gpt-4o-mini-2024-07-18", "gpt-4o-mini"),
    ("gpt-5-2-codex-medium", "gpt-5.2-codex-medium"),
    ("gpt-5-2-medium", "gpt-5.2"),
    ("gpt-5-2025-08-07", "gpt-5"),
    ("gpt-5-3-codex-high", "gpt-5.3-codex-high"),
    ("gpt-5-3-codex-high-priority", "gpt-5.3-codex-high-fast"),
    ("gpt-5-3-codex-low", "gpt-5.3-codex-low"),
    ("gpt-5-3-codex-low-priority", "gpt-5.3-codex-low-fast"),
    ("gpt-5-3-codex-medium", "gpt-5.3-codex"),
    ("gpt-5-3-codex-medium-priority", "gpt-5.3-codex-medium-fast"),
    ("gpt-5-3-codex-xhigh", "gpt-5.3-codex-xhigh"),
    ("gpt-5-3-codex-xhigh-priority", "gpt-5.3-codex-xhigh-fast"),
    ("gpt-5-4-high", "gpt-5.4-high"),
    ("gpt-5-4-low", "gpt-5.4-low"),
    ("gpt-5-4-medium", "gpt-5.4-medium"),
    ("gpt-5-4-mini-high", "gpt-5.4-mini-high"),
    ("gpt-5-4-mini-low", "gpt-5.4-mini-low"),
    ("gpt-5-4-mini-medium", "gpt-5.4-mini-medium"),
    ("gpt-5-4-mini-xhigh", "gpt-5.4-mini-xhigh"),
    ("gpt-5-4-none", "gpt-5.4-none"),
    ("gpt-5-4-xhigh", "gpt-5.4-xhigh"),
    ("gpt-5-5", "gpt-5.5-medium"),
    ("gpt-5-5-high", "gpt-5.5-high"),
    ("gpt-5-5-high-priority", "gpt-5.5-high-fast"),
    ("gpt-5-5-low", "gpt-5.5-low"),
    ("gpt-5-5-low-priority", "gpt-5.5-low-fast"),
    ("gpt-5-5-medium", "gpt-5.5-medium"),
    ("gpt-5-5-medium-priority", "gpt-5.5-medium-fast"),
    ("gpt-5-5-none", "gpt-5.5-none"),
    ("gpt-5-5-none-priority", "gpt-5.5-none-fast"),
    ("gpt-5-5-xhigh", "gpt-5.5-xhigh"),
    ("gpt-5-5-xhigh-priority", "gpt-5.5-xhigh-fast"),
    ("gpt-5-pro-2025-10-06", "gpt-5-high"),
    ("gpt-5.2-codex", "gpt-5.2-codex-medium"),
    ("gpt-5.2-medium", "gpt-5.2"),
    ("gpt-5.3-codex-medium", "gpt-5.3-codex"),
    ("gpt-5.4", "gpt-5.4-medium"),
    ("gpt-5.5", "gpt-5.5-medium"),
    ("haiku-4.5", "claude-4.5-haiku"),
    ("kimi-k2-5", "kimi-k2.5"),
    ("minimax-m2-5", "minimax-m2.5"),
    ("model_claude_4_5_sonnet", "claude-4.5-sonnet"),
    ("model_claude_4_5_sonnet_thinking", "claude-4.5-sonnet-thinking"),
    ("o4.7", "claude-opus-4-7-medium"),
    ("opus-4", "claude-4-opus"),
    ("opus-4-7", "claude-opus-4-7-medium"),
    ("opus-4.1", "claude-4.1-opus"),
    ("opus-4.6", "claude-opus-4.6"),
    ("opus-4.6-thinking", "claude-opus-4.6-thinking"),
    ("opus-4.7", "claude-opus-4-7-medium"),
    ("opus-4.7-thinking", "claude-opus-4-7-medium-thinking"),
    ("sonnet-3.5", "claude-3.5-sonnet"),
    ("sonnet-3.7", "claude-3.7-sonnet"),
    ("sonnet-4", "claude-4-sonnet"),
    ("sonnet-4.5", "claude-4.5-sonnet"),
    ("sonnet-4.5-thinking", "claude-4.5-sonnet-thinking"),
    ("sonnet-4.6", "claude-sonnet-4.6"),
    ("sonnet-4.6-1m", "claude-sonnet-4.6-1m"),
    ("sonnet-4.6-thinking", "claude-sonnet-4.6-thinking"),
    ("swe-1-6", "swe-1.6"),
    ("swe-1-6-fast", "swe-1.6-fast"),
    ("ws-haiku", "claude-4.5-haiku"),
    ("ws-opus", "claude-opus-4.6"),
    ("ws-opus-thinking", "claude-opus-4.6-thinking"),
    ("ws-sonnet", "claude-sonnet-4.6"),
    ("ws-sonnet-thinking", "claude-sonnet-4.6-thinking"),
];

pub fn windsurf_models() -> &'static [WindsurfModel] {
    MODELS
}

pub fn resolve_windsurf_model(name: &str) -> Option<WindsurfModel> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    let canonical = ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == normalized).then_some(*canonical))
        .unwrap_or(normalized.as_str());
    MODELS
        .iter()
        .find(|model| model_matches(model, canonical))
        .cloned()
}

fn model_matches(model: &WindsurfModel, value: &str) -> bool {
    model.canonical_name.eq_ignore_ascii_case(value)
        || model
            .model_uid
            .is_some_and(|uid| uid.eq_ignore_ascii_case(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_gpt55_cloud_alias_to_windsurf_model_uid() {
        let model = resolve_windsurf_model("gpt-5-5-low").expect("model should resolve");

        assert_eq!(model.canonical_name, "gpt-5.5-low");
        assert_eq!(model.model_uid, Some("gpt-5-5-low"));
        assert_eq!(model.enum_value, 0);
        assert_eq!(model.credit_multiplier, 1.0);
    }

    #[test]
    fn resolves_claude_opus_47_bare_alias_to_medium() {
        let model = resolve_windsurf_model("claude-opus-4.7").expect("model should resolve");

        assert_eq!(model.canonical_name, "claude-opus-4-7-medium");
        assert_eq!(model.model_uid, Some("claude-opus-4-7-medium"));
        assert_eq!(model.enum_value, 0);
        assert_eq!(model.credit_multiplier, 8.0);
    }

    #[test]
    fn resolves_priority_alias_to_fast_variant() {
        let model = resolve_windsurf_model("gpt-5-5-low-priority").expect("model should resolve");

        assert_eq!(model.canonical_name, "gpt-5.5-low-fast");
        assert_eq!(model.model_uid, Some("gpt-5-5-low-priority"));
        assert_eq!(model.credit_multiplier, 2.0);
    }

    #[test]
    fn resolves_full_gpt55_effort_ladder_and_priority_aliases() {
        let none = resolve_windsurf_model("gpt-5-5-none").expect("none should resolve");
        assert_eq!(none.canonical_name, "gpt-5.5-none");
        assert_eq!(none.model_uid, Some("gpt-5-5-none"));
        assert_eq!(none.credit_multiplier, 1.0);

        let high = resolve_windsurf_model("gpt-5.5-high").expect("high should resolve");
        assert_eq!(high.canonical_name, "gpt-5.5-high");
        assert_eq!(high.model_uid, Some("gpt-5-5-high"));
        assert_eq!(high.credit_multiplier, 4.0);

        let xhigh_fast = resolve_windsurf_model("gpt-5-5-xhigh-priority")
            .expect("xhigh priority should resolve");
        assert_eq!(xhigh_fast.canonical_name, "gpt-5.5-xhigh-fast");
        assert_eq!(xhigh_fast.model_uid, Some("gpt-5-5-xhigh-priority"));
        assert_eq!(xhigh_fast.credit_multiplier, 16.0);
    }

    #[test]
    fn resolves_windsurfapi_catalog_aliases_beyond_gpt55() {
        let gpt52_medium = resolve_windsurf_model("gpt-5.2-medium").expect("gpt-5.2 medium alias");
        assert_eq!(gpt52_medium.canonical_name, "gpt-5.2");
        assert_eq!(gpt52_medium.model_uid, Some("MODEL_GPT_5_2_MEDIUM"));

        let haiku = resolve_windsurf_model("claude-haiku-4-5-20251001").expect("dated haiku alias");
        assert_eq!(haiku.canonical_name, "claude-4.5-haiku");
        assert_eq!(haiku.model_uid, Some("MODEL_PRIVATE_11"));

        let uid = resolve_windsurf_model("MODEL_GPT_5_2_LOW").expect("model uid alias");
        assert_eq!(uid.canonical_name, "gpt-5.2-low");
        assert_eq!(uid.enum_value, 400);

        let cursor = resolve_windsurf_model("ws-opus").expect("cursor alias");
        assert_eq!(cursor.canonical_name, "claude-opus-4.6");
    }

    #[test]
    fn windsurf_catalog_covers_current_windsurfapi_model_set() {
        assert_eq!(MODELS.len(), 139);
        assert!(ALIASES.len() >= 100);
    }
}
