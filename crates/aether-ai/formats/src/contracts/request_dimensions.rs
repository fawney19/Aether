use serde::{Deserialize, Serialize};

/// Client behavior profile detected at the ingress boundary.
///
/// This is deliberately independent from the credential carrier: an
/// Anthropic SDK may use bearer auth, and Claude Code may be authenticated by
/// an Aether API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientSurface {
    ClaudeCode,
    AnthropicSdk,
    GenericCompatible,
}

impl ClientSurface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::AnthropicSdk => "anthropic_sdk",
            Self::GenericCompatible => "generic_compatible",
        }
    }
}

/// Semantic operation carried over an API wire format.
///
/// Operations must not be represented as additional API formats: both
/// Anthropic message creation and token counting use the `claude:messages`
/// request contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiOperation {
    ClaudeMessagesCreate,
    ClaudeCountTokens,
    OpenAiResponsesCompact,
    OpenAiMemoriesSummarize,
}

impl ApiOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeMessagesCreate => "messages",
            Self::ClaudeCountTokens => "count_tokens",
            Self::OpenAiResponsesCompact => "compact",
            Self::OpenAiMemoriesSummarize => "trace_summarize",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiOperation, ClientSurface};

    #[test]
    fn request_dimensions_have_stable_external_names() {
        assert_eq!(ClientSurface::ClaudeCode.as_str(), "claude_code");
        assert_eq!(ApiOperation::ClaudeMessagesCreate.as_str(), "messages");
        assert_eq!(ApiOperation::ClaudeCountTokens.as_str(), "count_tokens");
        assert_eq!(
            ApiOperation::OpenAiMemoriesSummarize.as_str(),
            "trace_summarize"
        );
    }
}
