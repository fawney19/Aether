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
            "openai:responses" | "openai:cli" | "/v1/responses" => Ok(Self::OpenAiResponses),
            "openai:responses:compact" | "openai:compact" | "/v1/responses/compact" => {
                Ok(Self::OpenAiResponsesCompact)
            }
            "claude:messages" | "claude:chat" | "claude:cli" | "/v1/messages" => {
                Ok(Self::ClaudeMessages)
            }
            "gemini:generate_content" | "gemini:chat" | "gemini:cli" => {
                Ok(Self::GeminiGenerateContent)
            }
            _ => Err(()),
        }
    }
}

pub fn normalize_legacy_openai_format_alias(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai:cli" => "openai:responses".to_string(),
        "openai:compact" => "openai:responses:compact".to_string(),
        other => other.to_string(),
    }
}

pub fn legacy_openai_format_alias_matches(left: &str, right: &str) -> bool {
    normalize_legacy_openai_format_alias(left) == normalize_legacy_openai_format_alias(right)
}

pub fn openai_format_storage_aliases(value: &str) -> Vec<String> {
    match normalize_legacy_openai_format_alias(value).as_str() {
        "openai:responses" => vec!["openai:responses".to_string(), "openai:cli".to_string()],
        "openai:responses:compact" => vec![
            "openai:responses:compact".to_string(),
            "openai:compact".to_string(),
        ],
        normalized => vec![normalized.to_string()],
    }
}

pub fn is_openai_responses_format(value: &str) -> bool {
    normalize_legacy_openai_format_alias(value) == "openai:responses"
}

pub fn is_openai_responses_compact_format(value: &str) -> bool {
    normalize_legacy_openai_format_alias(value) == "openai:responses:compact"
}

pub fn is_openai_responses_family_format(value: &str) -> bool {
    matches!(
        normalize_legacy_openai_format_alias(value).as_str(),
        "openai:responses" | "openai:responses:compact"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        legacy_openai_format_alias_matches, normalize_legacy_openai_format_alias,
        openai_format_storage_aliases, FormatId,
    };

    #[test]
    fn normalizes_legacy_aliases() {
        assert_eq!(
            FormatId::parse("openai:cli"),
            Some(FormatId::OpenAiResponses)
        );
        assert_eq!(
            FormatId::parse("openai:compact"),
            Some(FormatId::OpenAiResponsesCompact)
        );
        assert_eq!(
            FormatId::parse("claude:cli"),
            Some(FormatId::ClaudeMessages)
        );
        assert_eq!(
            FormatId::parse("gemini:chat"),
            Some(FormatId::GeminiGenerateContent)
        );
    }

    #[test]
    fn normalizes_openai_legacy_aliases_without_rewriting_other_families() {
        assert_eq!(
            normalize_legacy_openai_format_alias(" openai:cli "),
            "openai:responses"
        );
        assert_eq!(
            normalize_legacy_openai_format_alias("OPENAI:COMPACT"),
            "openai:responses:compact"
        );
        assert_eq!(
            normalize_legacy_openai_format_alias("claude:cli"),
            "claude:cli"
        );
        assert!(legacy_openai_format_alias_matches(
            "openai:cli",
            "openai:responses"
        ));
    }

    #[test]
    fn storage_aliases_include_legacy_openai_rows_only() {
        assert_eq!(
            openai_format_storage_aliases("openai:responses"),
            vec!["openai:responses".to_string(), "openai:cli".to_string()]
        );
        assert_eq!(
            openai_format_storage_aliases("openai:compact"),
            vec![
                "openai:responses:compact".to_string(),
                "openai:compact".to_string()
            ]
        );
        assert_eq!(
            openai_format_storage_aliases("claude:cli"),
            vec!["claude:cli".to_string()]
        );
    }
}
