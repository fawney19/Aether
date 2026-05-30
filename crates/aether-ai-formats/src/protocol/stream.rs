use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalUsage {
    pub input_tokens: u64,
    /// True when `input_tokens` already includes cache read and cache creation
    /// input tokens. Claude-style usage leaves cached input tokens separate.
    #[serde(default, skip_serializing_if = "is_false")]
    pub input_tokens_include_cache: bool,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_creation_ephemeral_5m_tokens: u64,
    pub cache_creation_ephemeral_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub reasoning_tokens: u64,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanonicalContentPart {
    ImageUrl(String),
    File {
        file_data: Option<String>,
        reference: Option<String>,
        mime_type: Option<String>,
        filename: Option<String>,
    },
    Audio {
        data: String,
        format: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanonicalStreamEvent {
    Start,
    TextDelta(String),
    ReasoningDelta(String),
    ReasoningSummaryDone,
    ReasoningSignature(String),
    ContentPart(CanonicalContentPart),
    ImageGenerationCall {
        index: usize,
        item: Value,
    },
    ToolCallStart {
        index: usize,
        call_id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        index: usize,
        arguments: String,
    },
    ToolResultDelta {
        index: usize,
        tool_use_id: String,
        name: Option<String>,
        content: String,
    },
    UnknownEvent(Value),
    Finish {
        finish_reason: Option<String>,
        usage: Option<CanonicalUsage>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalStreamFrame {
    pub id: String,
    pub model: String,
    pub event: CanonicalStreamEvent,
}
