//! Format identity and aliases.

use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FormatFamily {
    OpenAi,
    Claude,
    Gemini,
    Jina,
    Doubao,
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
    OpenAiEmbedding,
    OpenAiRerank,
    ClaudeMessages,
    GeminiGenerateContent,
    GeminiEmbedding,
    JinaEmbedding,
    JinaRerank,
    DoubaoEmbedding,
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
            Self::OpenAiChat
            | Self::OpenAiResponses
            | Self::OpenAiResponsesCompact
            | Self::OpenAiEmbedding
            | Self::OpenAiRerank => FormatFamily::OpenAi,
            Self::ClaudeMessages => FormatFamily::Claude,
            Self::GeminiGenerateContent | Self::GeminiEmbedding => FormatFamily::Gemini,
            Self::JinaEmbedding | Self::JinaRerank => FormatFamily::Jina,
            Self::DoubaoEmbedding => FormatFamily::Doubao,
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
            Self::OpenAiEmbedding => "openai:embedding",
            Self::OpenAiRerank => "openai:rerank",
            Self::ClaudeMessages => "claude:messages",
            Self::GeminiGenerateContent => "gemini:generate_content",
            Self::GeminiEmbedding => "gemini:embedding",
            Self::JinaEmbedding => "jina:embedding",
            Self::JinaRerank => "jina:rerank",
            Self::DoubaoEmbedding => "doubao:embedding",
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
            "openai:embedding" | "/v1/embeddings" => Ok(Self::OpenAiEmbedding),
            "openai:rerank" | "/v1/rerank" => Ok(Self::OpenAiRerank),
            "claude:messages" | "/v1/messages" => Ok(Self::ClaudeMessages),
            "gemini:generate_content" => Ok(Self::GeminiGenerateContent),
            "gemini:embedding" => Ok(Self::GeminiEmbedding),
            "jina:embedding" | "/jina/v1/embeddings" => Ok(Self::JinaEmbedding),
            "jina:rerank" | "/jina/v1/rerank" => Ok(Self::JinaRerank),
            "doubao:embedding" => Ok(Self::DoubaoEmbedding),
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

pub fn api_format_uses_body_stream_field(value: &str) -> bool {
    matches!(
        FormatId::parse(value).map(FormatId::canonical),
        Some(FormatId::OpenAiChat | FormatId::OpenAiResponses | FormatId::ClaudeMessages)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        api_format_alias_matches, api_format_storage_aliases, api_format_uses_body_stream_field,
        normalize_api_format_alias, FormatId,
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
    fn parses_embedding_api_formats() {
        assert_eq!(
            FormatId::parse("openai:embedding"),
            Some(FormatId::OpenAiEmbedding)
        );
        assert_eq!(
            FormatId::parse("/v1/embeddings"),
            Some(FormatId::OpenAiEmbedding)
        );
        assert_eq!(
            FormatId::parse("gemini:embedding"),
            Some(FormatId::GeminiEmbedding)
        );
        assert_eq!(
            FormatId::parse("jina:embedding"),
            Some(FormatId::JinaEmbedding)
        );
        assert_eq!(
            FormatId::parse("/jina/v1/embeddings"),
            Some(FormatId::JinaEmbedding)
        );
        assert_eq!(
            FormatId::parse("doubao:embedding"),
            Some(FormatId::DoubaoEmbedding)
        );
        assert_eq!(FormatId::OpenAiEmbedding.to_string(), "openai:embedding");
    }

    #[test]
    fn embedding_format_ids_keep_provider_family_and_default_profile() {
        use super::{FormatFamily, FormatProfile};

        for (format, family) in [
            (FormatId::OpenAiEmbedding, FormatFamily::OpenAi),
            (FormatId::GeminiEmbedding, FormatFamily::Gemini),
            (FormatId::JinaEmbedding, FormatFamily::Jina),
            (FormatId::DoubaoEmbedding, FormatFamily::Doubao),
        ] {
            assert_eq!(format.family(), family);
            assert_eq!(format.profile(), FormatProfile::Default);
            assert_eq!(FormatId::parse(format.as_str()), Some(format));
            assert_eq!(format.to_string(), format.as_str());
        }
    }

    #[test]
    fn parses_rerank_api_formats() {
        assert_eq!(
            FormatId::parse("openai:rerank"),
            Some(FormatId::OpenAiRerank)
        );
        assert_eq!(FormatId::parse("/v1/rerank"), Some(FormatId::OpenAiRerank));
        assert_eq!(FormatId::parse("jina:rerank"), Some(FormatId::JinaRerank));
        assert_eq!(
            FormatId::parse("/jina/v1/rerank"),
            Some(FormatId::JinaRerank)
        );
        assert_eq!(FormatId::OpenAiRerank.to_string(), "openai:rerank");
    }

    #[test]
    fn rerank_format_ids_keep_provider_family_and_default_profile() {
        use super::{FormatFamily, FormatProfile};

        for (format, family) in [
            (FormatId::OpenAiRerank, FormatFamily::OpenAi),
            (FormatId::JinaRerank, FormatFamily::Jina),
        ] {
            assert_eq!(format.family(), family);
            assert_eq!(format.profile(), FormatProfile::Default);
            assert_eq!(FormatId::parse(format.as_str()), Some(format));
            assert_eq!(format.to_string(), format.as_str());
        }
    }

    #[test]
    fn rejects_unknown_embedding_format() {
        assert_eq!(FormatId::parse("embedding"), None);
        assert_eq!(FormatId::parse("openai:embeddings"), None);
        assert_eq!(FormatId::parse("claude:embedding"), None);
        assert_eq!(FormatId::parse("gemini:embed_content"), None);
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
        assert_eq!(
            normalize_api_format_alias("OPENAI:EMBEDDING"),
            "openai:embedding"
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
        assert_eq!(
            normalize_api_format_alias("openai:compact"),
            "openai:compact"
        );
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
        assert_eq!(
            api_format_storage_aliases("openai:embedding"),
            vec!["openai:embedding".to_string()]
        );
        assert_eq!(
            api_format_storage_aliases("gemini:embedding"),
            vec!["gemini:embedding".to_string()]
        );
        assert_eq!(
            api_format_storage_aliases("jina:embedding"),
            vec!["jina:embedding".to_string()]
        );
        assert_eq!(
            api_format_storage_aliases("doubao:embedding"),
            vec!["doubao:embedding".to_string()]
        );
    }

    #[test]
    fn body_stream_field_support_matches_provider_wire_formats() {
        assert!(api_format_uses_body_stream_field("openai:chat"));
        assert!(api_format_uses_body_stream_field("/v1/chat/completions"));
        assert!(api_format_uses_body_stream_field("openai:responses"));
        assert!(api_format_uses_body_stream_field("/v1/responses"));
        assert!(api_format_uses_body_stream_field("claude:messages"));
        assert!(api_format_uses_body_stream_field("/v1/messages"));
        assert!(!api_format_uses_body_stream_field(
            "openai:responses:compact"
        ));
        assert!(!api_format_uses_body_stream_field("/v1/responses/compact"));
        assert!(!api_format_uses_body_stream_field(
            "gemini:generate_content"
        ));
        assert!(!api_format_uses_body_stream_field("openai:embedding"));
    }
}
