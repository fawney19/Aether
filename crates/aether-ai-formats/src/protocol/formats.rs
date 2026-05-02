//! Format identity and per-wire-format adapters.
//!
//! Each child module owns the boundary between one external wire shape and
//! the canonical IR. Registry conversion is intentionally constrained to:
//! source format -> canonical -> target format.

use std::{fmt, str::FromStr};

pub mod claude_messages;
pub mod gemini_generate_content;
pub mod openai_chat;
pub mod openai_responses;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FormatFamily {
    OpenAi,
    Claude,
    Gemini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FormatProfile {
    Default,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FormatId {
    OpenAiChat,
    OpenAiResponses,
    OpenAiResponsesCompact,
    ClaudeMessages,
    GeminiGenerateContent,
}

impl FormatId {
    pub fn parse(value: &str) -> Option<Self> {
        value.parse().ok()
    }

    pub fn canonical(self) -> Self {
        self
    }

    pub fn family(self) -> FormatFamily {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses | Self::OpenAiResponsesCompact => {
                FormatFamily::OpenAi
            }
            Self::ClaudeMessages => FormatFamily::Claude,
            Self::GeminiGenerateContent => FormatFamily::Gemini,
        }
    }

    pub fn profile(self) -> FormatProfile {
        match self {
            Self::OpenAiResponsesCompact => FormatProfile::Compact,
            _ => FormatProfile::Default,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai:chat",
            Self::OpenAiResponses => "openai:responses",
            Self::OpenAiResponsesCompact => "openai:responses:compact",
            Self::ClaudeMessages => "claude:messages",
            Self::GeminiGenerateContent => "gemini:generate_content",
        }
    }
}

impl fmt::Display for FormatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FormatId {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai:chat" | "/v1/chat/completions" => Ok(Self::OpenAiChat),
            "openai:responses" | "/v1/responses" => Ok(Self::OpenAiResponses),
            "openai:responses:compact" | "/v1/responses/compact" => {
                Ok(Self::OpenAiResponsesCompact)
            }
            "claude:messages" | "/v1/messages" => Ok(Self::ClaudeMessages),
            "gemini:generate_content" => Ok(Self::GeminiGenerateContent),
            _ => Err(()),
        }
    }
}

pub fn normalize_api_format_alias(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub fn api_format_alias_matches(left: &str, right: &str) -> bool {
    normalize_api_format_alias(left) == normalize_api_format_alias(right)
}

pub fn api_format_storage_aliases(value: &str) -> Vec<String> {
    vec![normalize_api_format_alias(value)]
}

pub fn is_openai_responses_format(value: &str) -> bool {
    normalize_api_format_alias(value) == "openai:responses"
}

pub fn is_openai_responses_compact_format(value: &str) -> bool {
    normalize_api_format_alias(value) == "openai:responses:compact"
}

pub fn is_openai_responses_family_format(value: &str) -> bool {
    matches!(
        normalize_api_format_alias(value).as_str(),
        "openai:responses" | "openai:responses:compact"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        api_format_alias_matches, api_format_storage_aliases, normalize_api_format_alias, FormatId,
    };

    #[test]
    fn retired_api_formats_do_not_parse() {
        assert_eq!(FormatId::parse("openai:cli"), None);
        assert_eq!(FormatId::parse("openai:compact"), None);
        assert_eq!(FormatId::parse("claude:chat"), None);
        assert_eq!(FormatId::parse("claude:cli"), None);
        assert_eq!(FormatId::parse("gemini:chat"), None);
        assert_eq!(FormatId::parse("gemini:cli"), None);
    }

    #[test]
    fn normalizes_api_format_aliases() {
        assert_eq!(
            normalize_api_format_alias(" OPENAI:RESPONSES "),
            "openai:responses"
        );
        assert_eq!(
            normalize_api_format_alias("OPENAI:RESPONSES:COMPACT"),
            "openai:responses:compact"
        );
        assert_eq!(
            normalize_api_format_alias("CLAUDE:MESSAGES"),
            "claude:messages"
        );
        assert_eq!(
            normalize_api_format_alias("GEMINI:GENERATE_CONTENT"),
            "gemini:generate_content"
        );
        assert_eq!(normalize_api_format_alias("openai:image"), "openai:image");
        assert_eq!(normalize_api_format_alias("openai:video"), "openai:video");
        assert_eq!(normalize_api_format_alias("gemini:video"), "gemini:video");
        assert_eq!(normalize_api_format_alias("gemini:files"), "gemini:files");
        assert!(!api_format_alias_matches("claude:cli", "claude:messages"));
        assert!(!api_format_alias_matches(
            "gemini:chat",
            "gemini:generate_content"
        ));
        assert!(!api_format_alias_matches("openai:cli", "openai:responses"));
    }

    #[test]
    fn storage_aliases_only_include_normalized_value() {
        assert_eq!(
            api_format_storage_aliases("openai:responses"),
            vec!["openai:responses".to_string()]
        );
        assert_eq!(
            api_format_storage_aliases("openai:responses:compact"),
            vec!["openai:responses:compact".to_string()]
        );
        assert_eq!(
            api_format_storage_aliases("claude:messages"),
            vec!["claude:messages".to_string()]
        );
        assert_eq!(
            api_format_storage_aliases("gemini:generate_content"),
            vec!["gemini:generate_content".to_string()]
        );
    }
}
