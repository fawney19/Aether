use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::formats::openai::shared::map_thinking_budget_to_openai_reasoning_effort;
use crate::formats::shared::model_directives::ReasoningEffort;
use crate::formats::shared::response::remove_empty_pages_from_tool_input_value;

pub use crate::protocol::stream::{CanonicalStreamEvent, CanonicalStreamFrame};

pub(crate) const OPENAI_RESPONSES_EXTENSION_NAMESPACE: &str = "openai_responses";
pub(crate) const OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE: &str = "openai_cli";
const AETHER_EXTENSION_NAMESPACE: &str = "aether";
const CLAUDE_MESSAGES_REQUEST_SOURCE_MARKER: &str = "claude_messages_request";
const CLAUDE_SYSTEM_SOURCE_MARKER: &str = "claude_system";
const CLAUDE_THINKING_SOURCE_MARKER: &str = "claude_thinking";
const CLAUDE_TOOL_RESULT_SOURCE_MARKER: &str = "claude_tool_result";
const CLAUDE_RAW_SOURCE_MARKER: &str = "claude_raw";
const OPENAI_THINKING_SOURCE_MARKER: &str = "openai_thinking";
const OPENAI_CUSTOM_TOOL_CALL_SOURCE_MARKER: &str = "openai_custom_tool_call";
const OPENAI_OUTPUT_AUDIO_SOURCE_MARKER: &str = "openai_output_audio";
const OPENAI_CHAT_TOOL_RESULT_SOURCE_MARKER: &str = "openai_chat_tool_result";
const OPENAI_RESPONSES_TOOL_RESULT_SOURCE_MARKER: &str = "openai_responses_tool_result";
const OPENAI_RESPONSES_INPUT_MESSAGE_SOURCE_MARKER: &str = "openai_responses_input_message";
const OPENAI_RESPONSES_RAW_SOURCE_MARKER: &str = "openai_responses_raw";
const OPENAI_RESPONSES_RAW_CONTENT_SOURCE_MARKER: &str = "openai_responses_raw_content";
const OPENAI_RESPONSES_CONTENT_MARKER: &str = "openai_responses_content";
const OPENAI_CHAT_TOOL_ERROR_PREFIX: &str = "[tool error]";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalRole {
    User,
    Assistant,
    System,
    Developer,
    Tool,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalStopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
    ContentFiltered,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanonicalToolChoice {
    Auto,
    None,
    Required,
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CanonicalContentBlock {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
    File {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
    Audio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_text: Option<String>,
        #[serde(default)]
        is_error: bool,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
    Unknown {
        raw_type: String,
        payload: Value,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extensions: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalInstruction {
    pub role: CanonicalRole,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalMessage {
    pub role: CanonicalRole,
    #[serde(default)]
    pub content: Vec<CanonicalContentBlock>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CanonicalGenerationConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalThinkingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalResponseFormat {
    pub format_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CanonicalUsage {
    #[serde(default)]
    pub input_tokens: u64,
    /// True when `input_tokens` already includes cache read and cache creation
    /// input tokens. Claude-style usage leaves cached input tokens separate.
    #[serde(default, skip_serializing_if = "is_false")]
    pub input_tokens_include_cache: bool,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub cache_creation_ephemeral_5m_tokens: u64,
    #[serde(default)]
    pub cache_creation_ephemeral_1h_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CanonicalEmbeddingInput {
    String(String),
    StringArray(Vec<String>),
    TokenArray(Vec<i64>),
    TokenArrayArray(Vec<Vec<i64>>),
    Multimodal(Vec<CanonicalEmbeddingContent>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalEmbeddingContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_images: Option<Vec<String>>,
}

impl CanonicalEmbeddingInput {
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::String(value) => value.trim().is_empty(),
            Self::StringArray(values) => {
                values.is_empty() || values.iter().any(|value| value.trim().is_empty())
            }
            Self::TokenArray(values) => values.is_empty(),
            Self::TokenArrayArray(values) => values.is_empty() || values.iter().any(Vec::is_empty),
            Self::Multimodal(values) => {
                values.is_empty() || values.iter().any(CanonicalEmbeddingContent::is_empty)
            }
        }
    }

    pub(crate) fn as_string_items(&self) -> Option<Vec<&str>> {
        match self {
            Self::String(value) => Some(vec![value.as_str()]),
            Self::StringArray(values) => Some(values.iter().map(String::as_str).collect()),
            Self::TokenArray(_) | Self::TokenArrayArray(_) | Self::Multimodal(_) => None,
        }
    }
}

impl CanonicalEmbeddingContent {
    pub(crate) fn is_empty(&self) -> bool {
        let text_empty = self
            .text
            .as_ref()
            .is_some_and(|value| value.trim().is_empty());
        let image_empty = self
            .image
            .as_ref()
            .is_some_and(|value| value.trim().is_empty());
        let video_empty = self
            .video
            .as_ref()
            .is_some_and(|value| value.trim().is_empty());
        let multi_images_empty = self.multi_images.as_ref().is_some_and(|values| {
            values.is_empty() || values.iter().any(|value| value.trim().is_empty())
        });
        let has_any = self
            .text
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .image
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .video
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            || self.multi_images.as_ref().is_some_and(|values| {
                !values.is_empty() && values.iter().all(|value| !value.trim().is_empty())
            });
        !has_any || text_empty || image_empty || video_empty || multi_images_empty
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalEmbeddingRequest {
    pub input: CanonicalEmbeddingInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRerankRequest {
    pub query: String,
    #[serde(default)]
    pub documents: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_n: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_documents: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

impl CanonicalRerankRequest {
    pub(crate) fn is_empty(&self) -> bool {
        self.query.trim().is_empty()
            || self.documents.is_empty()
            || self.documents.iter().any(rerank_document_is_empty)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalEmbedding {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub embedding: Vec<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalEmbeddingResponse {
    pub id: String,
    pub model: String,
    #[serde(default)]
    pub embeddings: Vec<CanonicalEmbedding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CanonicalUsage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub instructions: Vec<CanonicalInstruction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default)]
    pub messages: Vec<CanonicalMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<CanonicalEmbeddingRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank: Option<CanonicalRerankRequest>,
    #[serde(default)]
    pub generation: CanonicalGenerationConfig,
    #[serde(default)]
    pub tools: Vec<CanonicalToolDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<CanonicalToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<CanonicalThinkingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<CanonicalResponseFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalResponseOutput {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub role: CanonicalRole,
    #[serde(default)]
    pub content: Vec<CanonicalContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<CanonicalStopReason>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

impl Default for CanonicalResponseOutput {
    fn default() -> Self {
        Self {
            index: 0,
            role: CanonicalRole::Assistant,
            content: Vec::new(),
            stop_reason: None,
            extensions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalResponse {
    pub id: String,
    pub model: String,
    #[serde(default)]
    pub outputs: Vec<CanonicalResponseOutput>,
    #[serde(default)]
    pub content: Vec<CanonicalContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<CanonicalStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CanonicalUsage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

pub fn from_openai_chat_to_canonical_request(body_json: &Value) -> Option<CanonicalRequest> {
    crate::formats::openai::chat::request::from_raw(body_json)
}

pub fn canonical_to_openai_chat_request(canonical: &CanonicalRequest) -> Option<Value> {
    crate::formats::openai::chat::request::to(
        canonical,
        &crate::formats::context::FormatContext::default(),
    )
}

pub fn from_openai_responses_to_canonical_request(body_json: &Value) -> Option<CanonicalRequest> {
    crate::formats::openai::responses::request::from_raw(body_json)
}

pub(crate) fn canonical_to_openai_responses_request_with_profile(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
    compact: bool,
) -> Option<Value> {
    crate::formats::openai::responses::request::to_raw(
        canonical,
        mapped_model,
        upstream_is_stream,
        compact,
    )
}

pub fn canonical_to_openai_responses_request(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    canonical_to_openai_responses_request_with_profile(
        canonical,
        mapped_model,
        upstream_is_stream,
        false,
    )
}

pub fn canonical_to_openai_responses_compact_request(
    canonical: &CanonicalRequest,
    mapped_model: &str,
) -> Option<Value> {
    canonical_to_openai_responses_request_with_profile(canonical, mapped_model, false, true)
}

pub fn from_claude_to_canonical_request(body_json: &Value) -> Option<CanonicalRequest> {
    crate::formats::claude::messages::request::from_raw(body_json)
}

pub fn canonical_to_claude_request(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    crate::formats::claude::messages::request::to_raw(canonical, mapped_model, upstream_is_stream)
}

pub fn from_gemini_to_canonical_request(
    body_json: &Value,
    request_path: &str,
) -> Option<CanonicalRequest> {
    crate::formats::gemini::generate_content::request::from_raw(body_json, request_path)
}

pub fn canonical_to_gemini_request(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    crate::formats::gemini::generate_content::request::to_raw(
        canonical,
        mapped_model,
        upstream_is_stream,
    )
}

#[cfg(test)]
pub(crate) fn from_embedding_to_canonical_request(
    body_json: &Value,
    namespace: &str,
) -> Option<CanonicalRequest> {
    match namespace {
        "openai" => crate::formats::openai::embedding::request::from_namespace(body_json, "openai"),
        "jina" => crate::formats::openai::embedding::request::from_namespace(body_json, "jina"),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn canonical_to_embedding_request(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    namespace: &str,
) -> Option<Value> {
    let ctx = crate::formats::context::FormatContext::default().with_mapped_model(mapped_model);
    match namespace {
        "openai" => crate::formats::openai::embedding::request::to(canonical, &ctx),
        "jina" => crate::formats::jina::embedding::request::to(canonical, &ctx),
        "gemini" => crate::formats::gemini::embedding::request::to(canonical, &ctx),
        "doubao" => crate::formats::doubao::embedding::request::to(canonical, &ctx),
        "aliyun" => crate::formats::aliyun::embedding::request::to(canonical, &ctx),
        _ => None,
    }
}

pub fn from_openai_chat_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::formats::openai::chat::response::from_raw(body_json)
}

pub fn from_openai_responses_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::formats::openai::responses::response::from_raw(body_json)
}

pub fn from_claude_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::formats::claude::messages::response::from_raw(body_json)
}

pub fn from_gemini_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::formats::gemini::generate_content::response::from_raw(body_json)
}

pub fn canonical_to_openai_chat_response(canonical: &CanonicalResponse) -> Value {
    crate::formats::openai::chat::response::to_raw(canonical)
}

pub(crate) fn canonical_blocks_to_openai_chat_message(content: &[CanonicalContentBlock]) -> Value {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    let mut visible_blocks = Vec::new();
    let mut reasoning_text = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut annotations = Vec::new();
    let mut refusal = Vec::new();
    let mut audio = None;
    let mut text_offset = 0_i64;
    for block in content {
        match block {
            CanonicalContentBlock::Thinking {
                text,
                signature,
                encrypted_content,
                extensions,
            } => {
                if let Some(data) = encrypted_content.as_ref().filter(|value| !value.is_empty()) {
                    reasoning_parts.push(json!({
                        "type": "redacted_thinking",
                        "data": data,
                    }));
                    continue;
                }
                if !text.trim().is_empty() {
                    let omit_reasoning_content = extensions
                        .get("openai")
                        .and_then(|value| value.get("omit_reasoning_content"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let omit_reasoning_parts = extensions
                        .get("openai")
                        .and_then(|value| value.get("omit_reasoning_parts"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !omit_reasoning_content {
                        reasoning_text.push(text.clone());
                    }
                    if !omit_reasoning_parts {
                        let mut reasoning_part = Map::new();
                        reasoning_part
                            .insert("type".to_string(), Value::String("thinking".to_string()));
                        reasoning_part.insert("thinking".to_string(), Value::String(text.clone()));
                        if let Some(signature) =
                            signature.as_ref().filter(|value| !value.is_empty())
                        {
                            reasoning_part
                                .insert("signature".to_string(), Value::String(signature.clone()));
                        }
                        reasoning_parts.push(Value::Object(reasoning_part));
                    }
                }
            }
            CanonicalContentBlock::ToolUse {
                id,
                name,
                input,
                extensions,
            } => tool_calls.push(canonical_tool_use_to_openai_chat_tool_call(
                id, name, input, extensions,
            )),
            CanonicalContentBlock::Audio {
                data, extensions, ..
            } if is_openai_output_audio_block(extensions) => {
                audio = canonical_audio_to_openai_chat_audio(data.as_deref(), extensions);
            }
            CanonicalContentBlock::Text { text, extensions } => {
                if let Some(raw_annotations) = extensions
                    .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
                    .or_else(|| extensions.get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE))
                    .and_then(|value| value.get("annotations"))
                    .and_then(Value::as_array)
                {
                    annotations.extend(raw_annotations.iter().map(|annotation| {
                        offset_openai_annotation_indices(annotation, text_offset)
                    }));
                }
                text_offset += text.chars().count() as i64;
                if let Some(part) = canonical_content_block_to_openai_part(block) {
                    visible_blocks.push(part);
                }
            }
            CanonicalContentBlock::Unknown {
                raw_type, payload, ..
            } if raw_type == "refusal" => {
                if let Some(text) = payload.get("refusal").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        refusal.push(text.to_string());
                    }
                }
            }
            other => {
                if let Some(part) = canonical_content_block_to_openai_part(other) {
                    if let Some(text) = part
                        .as_object()
                        .and_then(|object| object.get("text"))
                        .and_then(Value::as_str)
                    {
                        text_offset += text.chars().count() as i64;
                    }
                    visible_blocks.push(part);
                }
            }
        }
    }
    if !reasoning_text.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_text.join("")),
        );
    }
    if !reasoning_parts.is_empty() {
        message.insert("reasoning_parts".to_string(), Value::Array(reasoning_parts));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls.clone()));
    }
    if !refusal.is_empty() {
        message.insert("refusal".to_string(), Value::String(refusal.join("\n")));
    }
    if let Some(audio) = audio {
        message.insert("audio".to_string(), audio);
    }
    if !annotations.is_empty() {
        message.insert("annotations".to_string(), Value::Array(annotations));
    }
    message.insert(
        "content".to_string(),
        openai_content_value_from_parts(visible_blocks, !tool_calls.is_empty()),
    );
    Value::Object(message)
}

fn canonical_tool_use_to_openai_chat_tool_call(
    id: &str,
    name: &str,
    input: &Value,
    extensions: &BTreeMap<String, Value>,
) -> Value {
    if is_openai_custom_tool_call(extensions) {
        let mut custom = extensions
            .get("openai")
            .and_then(|value| value.get("custom"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        custom.insert("name".to_string(), Value::String(name.to_string()));
        custom.insert(
            "input".to_string(),
            Value::String(openai_custom_tool_input_text(input)),
        );
        return json!({
            "id": id,
            "type": "custom",
            "custom": Value::Object(custom),
        });
    }
    json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": canonicalize_tool_arguments(input),
        }
    })
}

pub(crate) fn canonical_tool_use_to_openai_responses_item(
    id: &str,
    name: &str,
    input: &Value,
    extensions: &BTreeMap<String, Value>,
) -> Value {
    if let Some(item_type) = openai_responses_hosted_tool_call_item_type(extensions) {
        let mut item = Map::new();
        item.insert("type".to_string(), Value::String(item_type.to_string()));
        item.insert("id".to_string(), Value::String(id.to_string()));
        item.insert("call_id".to_string(), Value::String(id.to_string()));
        item.insert("status".to_string(), Value::String("completed".to_string()));
        if let Some(input_object) = input.as_object() {
            for field in openai_responses_hosted_tool_input_fields(item_type) {
                if let Some(value) = input_object.get(*field) {
                    item.insert((*field).to_string(), value.clone());
                }
            }
        } else if item_type == "apply_patch_call" {
            item.insert("operation".to_string(), input.clone());
        } else if !name.trim().is_empty() {
            item.insert("name".to_string(), Value::String(name.to_string()));
        }
        let extension_fields = openai_responses_item_extension_object(extensions, &item);
        item.extend(extension_fields);
        return Value::Object(item);
    }
    if is_openai_custom_tool_call(extensions) {
        let item_id = openai_responses_tool_call_item_id(id, extensions);
        let mut item = Map::new();
        item.insert(
            "type".to_string(),
            Value::String("custom_tool_call".to_string()),
        );
        item.insert("id".to_string(), Value::String(item_id));
        item.insert("call_id".to_string(), Value::String(id.to_string()));
        item.insert("status".to_string(), Value::String("completed".to_string()));
        item.insert("name".to_string(), Value::String(name.to_string()));
        item.insert(
            "input".to_string(),
            Value::String(openai_custom_tool_input_text(input)),
        );
        let extension_fields = openai_responses_item_extension_object(extensions, &item);
        item.extend(extension_fields);
        return Value::Object(item);
    }
    let item_id = openai_responses_tool_call_item_id(id, extensions);
    let mut item = Map::new();
    item.insert(
        "type".to_string(),
        Value::String("function_call".to_string()),
    );
    item.insert("id".to_string(), Value::String(item_id));
    item.insert("call_id".to_string(), Value::String(id.to_string()));
    item.insert("name".to_string(), Value::String(name.to_string()));
    item.insert(
        "arguments".to_string(),
        Value::String(canonicalize_tool_arguments(input)),
    );
    let extension_fields = openai_responses_item_extension_object(extensions, &item);
    item.extend(extension_fields);
    Value::Object(item)
}

pub(crate) fn canonical_tool_use_to_openai_responses_input_item(
    id: &str,
    name: &str,
    input: &Value,
    extensions: &BTreeMap<String, Value>,
) -> Value {
    if let Some(item_type) = openai_responses_hosted_tool_call_item_type(extensions) {
        let mut item = Map::new();
        item.insert("type".to_string(), Value::String(item_type.to_string()));
        item.insert("id".to_string(), Value::String(id.to_string()));
        item.insert("call_id".to_string(), Value::String(id.to_string()));
        item.insert("status".to_string(), Value::String("completed".to_string()));
        if let Some(input_object) = input.as_object() {
            for field in openai_responses_hosted_tool_input_fields(item_type) {
                if let Some(value) = input_object.get(*field) {
                    item.insert((*field).to_string(), value.clone());
                }
            }
        } else if item_type == "apply_patch_call" {
            item.insert("operation".to_string(), input.clone());
        } else if !name.trim().is_empty() {
            item.insert("name".to_string(), Value::String(name.to_string()));
        }
        let extension_fields = openai_responses_item_extension_object(extensions, &item);
        item.extend(extension_fields);
        return Value::Object(item);
    }
    if is_openai_custom_tool_call(extensions) {
        let mut item = Map::new();
        item.insert(
            "type".to_string(),
            Value::String("custom_tool_call".to_string()),
        );
        if let Some(item_id) = openai_responses_request_tool_call_item_id(extensions, "ctc") {
            item.insert("id".to_string(), Value::String(item_id));
        }
        item.insert("call_id".to_string(), Value::String(id.to_string()));
        item.insert("status".to_string(), Value::String("completed".to_string()));
        item.insert("name".to_string(), Value::String(name.to_string()));
        item.insert(
            "input".to_string(),
            Value::String(openai_custom_tool_input_text(input)),
        );
        let extension_fields = openai_responses_item_extension_object(extensions, &item);
        item.extend(extension_fields);
        return Value::Object(item);
    }
    let mut item = Map::new();
    item.insert(
        "type".to_string(),
        Value::String("function_call".to_string()),
    );
    if let Some(item_id) = openai_responses_request_tool_call_item_id(extensions, "fc") {
        item.insert("id".to_string(), Value::String(item_id));
    }
    item.insert("call_id".to_string(), Value::String(id.to_string()));
    item.insert("name".to_string(), Value::String(name.to_string()));
    item.insert(
        "arguments".to_string(),
        Value::String(canonicalize_tool_arguments(input)),
    );
    let extension_fields = openai_responses_item_extension_object(extensions, &item);
    item.extend(extension_fields);
    Value::Object(item)
}

fn openai_responses_tool_call_item_id(
    call_id: &str,
    extensions: &BTreeMap<String, Value>,
) -> String {
    if let Some(item_id) = openai_responses_extension(extensions)
        .and_then(|value| value.get("item_id").or_else(|| value.get("id")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return item_id.to_string();
    }
    let trimmed = call_id.trim();
    if trimmed.is_empty() {
        "call_auto".to_string()
    } else {
        trimmed.to_string()
    }
}

fn openai_responses_request_tool_call_item_id(
    extensions: &BTreeMap<String, Value>,
    prefix: &str,
) -> Option<String> {
    openai_responses_extension(extensions)
        .and_then(|value| value.get("item_id").or_else(|| value.get("id")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with(prefix))
        .map(ToString::to_string)
}

fn openai_responses_hosted_tool_call_item_type(
    extensions: &BTreeMap<String, Value>,
) -> Option<&str> {
    let item_type = openai_responses_extension(extensions)
        .and_then(|value| value.get("item_type"))
        .and_then(Value::as_str)?;
    openai_responses_hosted_tool_name(item_type).map(|_| item_type)
}

fn openai_responses_hosted_tool_input_fields(item_type: &str) -> &'static [&'static str] {
    match item_type {
        "local_shell_call" | "shell_call" => &[
            "action",
            "environment",
            "status",
            "created_by",
            "max_output_length",
        ],
        "apply_patch_call" => &["operation", "status"],
        "computer_call" => &["action", "actions", "pending_safety_checks", "status"],
        _ => &[],
    }
}

fn openai_custom_tool_input_text(input: &Value) -> String {
    match input {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn canonical_audio_to_openai_chat_audio(
    data: Option<&str>,
    extensions: &BTreeMap<String, Value>,
) -> Option<Value> {
    let mut audio = extensions
        .get("openai")
        .and_then(|value| value.get("audio"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(data) = data.filter(|value| !value.is_empty()) {
        audio.insert("data".to_string(), Value::String(data.to_string()));
    }
    if !audio.contains_key("transcript") {
        if let Some(transcript) = openai_responses_extension(extensions)
            .and_then(|value| value.get("transcript"))
            .cloned()
        {
            audio.insert("transcript".to_string(), transcript);
        }
    }
    (!audio.is_empty()).then_some(Value::Object(audio))
}

pub(crate) fn canonical_to_openai_responses_response_with_profile(
    canonical: &CanonicalResponse,
    report_context: &Value,
    compact: bool,
) -> Value {
    crate::formats::openai::responses::response::to_raw(canonical, report_context, compact)
}

pub fn canonical_to_openai_responses_response(
    canonical: &CanonicalResponse,
    report_context: &Value,
) -> Value {
    canonical_to_openai_responses_response_with_profile(canonical, report_context, false)
}

pub fn canonical_to_openai_responses_compact_response(
    canonical: &CanonicalResponse,
    report_context: &Value,
) -> Value {
    canonical_to_openai_responses_response_with_profile(canonical, report_context, true)
}

pub fn canonical_to_claude_response(canonical: &CanonicalResponse) -> Value {
    crate::formats::claude::messages::response::to_raw(canonical)
}

pub fn canonical_to_gemini_response(
    canonical: &CanonicalResponse,
    report_context: &Value,
) -> Option<Value> {
    crate::formats::gemini::generate_content::response::to_raw(canonical, report_context)
}

pub fn from_embedding_to_canonical_response(
    body_json: &Value,
    namespace: &str,
) -> Option<CanonicalEmbeddingResponse> {
    match namespace {
        "openai" => {
            crate::formats::openai::embedding::response::from_namespace(body_json, "openai")
        }
        "jina" => crate::formats::openai::embedding::response::from_namespace(body_json, "jina"),
        "gemini" => crate::formats::gemini::embedding::response::from(body_json),
        "aliyun" => crate::formats::aliyun::embedding::response::from(body_json),
        _ => None,
    }
}

pub fn canonical_to_embedding_response(
    canonical: &CanonicalEmbeddingResponse,
    namespace: &str,
) -> Option<Value> {
    match namespace {
        "openai" => crate::formats::openai::embedding::response::to(canonical),
        "jina" => crate::formats::jina::embedding::response::to(canonical),
        _ => None,
    }
}

pub fn canonical_unknown_block_count(blocks: &[CanonicalContentBlock]) -> usize {
    blocks
        .iter()
        .filter(|block| matches!(block, CanonicalContentBlock::Unknown { .. }))
        .count()
}

pub fn canonical_request_unknown_block_count(request: &CanonicalRequest) -> usize {
    request
        .messages
        .iter()
        .map(|message| canonical_unknown_block_count(&message.content))
        .sum()
}

pub fn canonical_response_unknown_block_count(response: &CanonicalResponse) -> usize {
    canonical_unknown_block_count(&response.content)
}

pub(crate) fn openai_role_to_canonical(role: &str) -> CanonicalRole {
    match role.trim().to_ascii_lowercase().as_str() {
        "user" => CanonicalRole::User,
        "assistant" => CanonicalRole::Assistant,
        "system" => CanonicalRole::System,
        "developer" => CanonicalRole::Developer,
        "tool" | "function" => CanonicalRole::Tool,
        _ => CanonicalRole::Unknown,
    }
}

pub(crate) fn gemini_system_to_canonical_instructions(
    system_instruction: Option<&Value>,
) -> Option<Vec<CanonicalInstruction>> {
    let Some(system_instruction) = system_instruction else {
        return Some(Vec::new());
    };
    match system_instruction {
        Value::String(text) => {
            if text.trim().is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![CanonicalInstruction {
                    role: CanonicalRole::System,
                    text: text.clone(),
                    extensions: BTreeMap::new(),
                }])
            }
        }
        Value::Object(object) => {
            let parts = object.get("parts").and_then(Value::as_array)?;
            let mut instructions = Vec::new();
            for part in parts {
                let part = part.as_object()?;
                let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                instructions.push(CanonicalInstruction {
                    role: CanonicalRole::System,
                    text: text.to_string(),
                    extensions: gemini_extensions(part, &["text"]),
                });
            }
            Some(instructions)
        }
        _ => None,
    }
}

pub(crate) fn gemini_contents_to_canonical_messages(
    contents: Option<&Value>,
) -> Option<Vec<CanonicalMessage>> {
    let Some(contents) = contents else {
        return Some(Vec::new());
    };
    let contents = contents.as_array()?;
    let mut messages = Vec::new();
    for content in contents {
        let content_object = content.as_object()?;
        let role = match content_object
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "model" => CanonicalRole::Assistant,
            "system" => CanonicalRole::System,
            "tool" | "function" => CanonicalRole::Tool,
            _ => CanonicalRole::User,
        };
        let parts = content_object.get("parts").and_then(Value::as_array)?;
        let mut blocks = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            blocks.push(gemini_part_to_canonical_block(part, index)?);
        }
        if blocks.is_empty() {
            continue;
        }
        messages.push(CanonicalMessage {
            role,
            content: blocks,
            extensions: gemini_extensions(content_object, &["role", "parts"]),
        });
    }
    Some(messages)
}

pub(crate) fn gemini_part_to_canonical_block(
    part: &Value,
    index: usize,
) -> Option<CanonicalContentBlock> {
    let part_object = part.as_object()?;
    if let Some(text) = part_object.get("text").and_then(Value::as_str) {
        if part_object
            .get("thought")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Some(CanonicalContentBlock::Thinking {
                text: text.to_string(),
                signature: part_object
                    .get("thoughtSignature")
                    .or_else(|| part_object.get("thought_signature"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                encrypted_content: None,
                extensions: gemini_extensions(
                    part_object,
                    &["text", "thought", "thoughtSignature", "thought_signature"],
                ),
            });
        }
        return Some(CanonicalContentBlock::Text {
            text: text.to_string(),
            extensions: gemini_extensions(part_object, &["text"]),
        });
    }
    if let Some(inline_data) = part_object
        .get("inlineData")
        .or_else(|| part_object.get("inline_data"))
        .and_then(Value::as_object)
    {
        return gemini_inline_data_to_canonical_block(inline_data, part_object);
    }
    if let Some(file_data) = part_object
        .get("fileData")
        .or_else(|| part_object.get("file_data"))
        .and_then(Value::as_object)
    {
        return gemini_file_data_to_canonical_block(file_data, part_object);
    }
    if let Some(function_call) = part_object
        .get("functionCall")
        .or_else(|| part_object.get("function_call"))
        .and_then(Value::as_object)
    {
        let name = function_call
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let id = function_call
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("call_auto_{index}"));
        return Some(CanonicalContentBlock::ToolUse {
            id,
            name: name.to_string(),
            input: function_call
                .get("args")
                .cloned()
                .unwrap_or_else(|| json!({})),
            extensions: gemini_extensions(part_object, &["functionCall", "function_call"]),
        });
    }
    if let Some(function_response) = part_object
        .get("functionResponse")
        .or_else(|| part_object.get("function_response"))
        .and_then(Value::as_object)
    {
        let name = function_response
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let tool_use_id = function_response
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| name.clone())
            .unwrap_or_else(|| format!("toolu_response_{index}"));
        let response = function_response
            .get("response")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let output = match response {
            Value::Object(mut object) => object
                .remove("result")
                .unwrap_or_else(|| Value::Object(object)),
            other => other,
        };
        return Some(CanonicalContentBlock::ToolResult {
            tool_use_id,
            name,
            output: Some(output.clone()),
            content_text: Some(openai_responses_tool_output_text(&output)),
            is_error: false,
            extensions: gemini_extensions(part_object, &["functionResponse", "function_response"]),
        });
    }
    Some(CanonicalContentBlock::Unknown {
        raw_type: gemini_raw_part_type(part_object),
        payload: part.clone(),
        extensions: BTreeMap::from([("gemini".to_string(), part.clone())]),
    })
}

pub(crate) fn gemini_inline_data_to_canonical_block(
    inline_data: &Map<String, Value>,
    part_object: &Map<String, Value>,
) -> Option<CanonicalContentBlock> {
    let media_type = inline_data
        .get("mimeType")
        .or_else(|| inline_data.get("mime_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let data = inline_data
        .get("data")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    if media_type.starts_with("image/") {
        return Some(CanonicalContentBlock::Image {
            data: Some(data),
            url: None,
            media_type: Some(media_type),
            detail: None,
            extensions: gemini_extensions(part_object, &["inlineData", "inline_data"]),
        });
    }
    if let Some(format) = media_type.strip_prefix("audio/") {
        return Some(CanonicalContentBlock::Audio {
            data: Some(data),
            media_type: Some(media_type.clone()),
            format: Some(format.to_string()),
            extensions: gemini_extensions(part_object, &["inlineData", "inline_data"]),
        });
    }
    Some(CanonicalContentBlock::File {
        data: Some(data),
        file_id: None,
        file_url: None,
        media_type: Some(media_type),
        filename: None,
        extensions: gemini_extensions(part_object, &["inlineData", "inline_data"]),
    })
}

pub(crate) fn gemini_file_data_to_canonical_block(
    file_data: &Map<String, Value>,
    part_object: &Map<String, Value>,
) -> Option<CanonicalContentBlock> {
    let file_uri = file_data
        .get("fileUri")
        .or_else(|| file_data.get("file_uri"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let media_type = file_data
        .get("mimeType")
        .or_else(|| file_data.get("mime_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if media_type
        .as_deref()
        .is_some_and(|value| value.starts_with("image/"))
    {
        return Some(CanonicalContentBlock::Image {
            data: None,
            url: Some(file_uri),
            media_type,
            detail: None,
            extensions: gemini_extensions(part_object, &["fileData", "file_data"]),
        });
    }
    Some(CanonicalContentBlock::File {
        data: None,
        file_id: None,
        file_url: Some(file_uri),
        media_type,
        filename: None,
        extensions: gemini_extensions(part_object, &["fileData", "file_data"]),
    })
}

pub(crate) fn gemini_raw_part_type(part: &Map<String, Value>) -> String {
    for key in [
        "executableCode",
        "executable_code",
        "codeExecutionResult",
        "code_execution_result",
        "videoMetadata",
        "video_metadata",
    ] {
        if part.contains_key(key) {
            return key.to_string();
        }
    }
    "unknown".to_string()
}

pub(crate) fn claude_system_to_canonical_instructions(
    system: Option<&Value>,
) -> Option<Vec<CanonicalInstruction>> {
    let Some(system) = system else {
        return Some(Vec::new());
    };
    match system {
        Value::String(text) => {
            let text = strip_claude_billing_header(text);
            if text.trim().is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![CanonicalInstruction {
                    role: CanonicalRole::System,
                    text,
                    extensions: claude_system_instruction_extensions(BTreeMap::new()),
                }])
            }
        }
        Value::Array(blocks) => {
            let mut instructions = Vec::new();
            for block in blocks {
                let block = block.as_object()?;
                if block.get("type").and_then(Value::as_str).unwrap_or("text") != "text" {
                    continue;
                }
                let text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    instructions.push(CanonicalInstruction {
                        role: CanonicalRole::System,
                        text: strip_claude_billing_header(text),
                        extensions: claude_system_instruction_extensions(claude_extensions(
                            block,
                            &["type", "text"],
                        )),
                    });
                }
            }
            Some(instructions)
        }
        _ => None,
    }
}

fn claude_system_instruction_extensions(
    mut extensions: BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    canonical_extension_object_mut(&mut extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(CLAUDE_SYSTEM_SOURCE_MARKER.to_string()),
    );
    extensions
}

pub(crate) fn mark_claude_messages_request_source(extensions: &mut BTreeMap<String, Value>) {
    canonical_extension_object_mut(extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(CLAUDE_MESSAGES_REQUEST_SOURCE_MARKER.to_string()),
    );
}

pub(crate) fn is_claude_messages_request(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(CLAUDE_MESSAGES_REQUEST_SOURCE_MARKER)
}

pub(crate) fn is_claude_system_instruction(instruction: &CanonicalInstruction) -> bool {
    instruction
        .extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(CLAUDE_SYSTEM_SOURCE_MARKER)
}

pub(crate) fn claude_messages_to_canonical(
    messages: Option<&Value>,
) -> Option<Vec<CanonicalMessage>> {
    let Some(messages) = messages else {
        return Some(Vec::new());
    };
    let messages = messages.as_array()?;
    messages
        .iter()
        .map(|message| {
            let message = message.as_object()?;
            let role = openai_role_to_canonical(
                message
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            Some(CanonicalMessage {
                role,
                content: claude_content_to_canonical_blocks(message.get("content"))?,
                extensions: claude_extensions(message, &["role", "content"]),
            })
        })
        .collect()
}

pub(crate) fn claude_content_to_canonical_blocks(
    content: Option<&Value>,
) -> Option<Vec<CanonicalContentBlock>> {
    let Some(content) = content else {
        return Some(Vec::new());
    };
    match content {
        Value::Null => Some(Vec::new()),
        Value::String(text) => {
            if text.is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![CanonicalContentBlock::Text {
                    text: text.clone(),
                    extensions: BTreeMap::new(),
                }])
            }
        }
        Value::Array(blocks) => {
            let mut canonical = Vec::new();
            let mut next_generated_tool_use_index = 0usize;
            for block in blocks {
                let mut canonical_block = claude_block_to_canonical_block(block)?;
                if let CanonicalContentBlock::ToolUse { id, .. } = &mut canonical_block {
                    if id.trim().is_empty() {
                        *id = format!("toolu_auto_{next_generated_tool_use_index}");
                        next_generated_tool_use_index += 1;
                    }
                }
                canonical.push(canonical_block);
            }
            Some(canonical)
        }
        _ => None,
    }
}

pub(crate) fn claude_block_to_canonical_block(block: &Value) -> Option<CanonicalContentBlock> {
    let block_object = block.as_object()?;
    let raw_type = block_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match raw_type.as_str() {
        "text" => Some(CanonicalContentBlock::Text {
            text: block_object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            extensions: claude_extensions(block_object, &["type", "text"]),
        }),
        "thinking" => Some(CanonicalContentBlock::Thinking {
            text: block_object
                .get("thinking")
                .or_else(|| block_object.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            signature: block_object
                .get("signature")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            encrypted_content: None,
            extensions: claude_thinking_extensions(claude_extensions(
                block_object,
                &["type", "thinking", "text", "signature"],
            )),
        }),
        "redacted_thinking" => Some(CanonicalContentBlock::Thinking {
            text: String::new(),
            signature: None,
            encrypted_content: block_object
                .get("data")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            extensions: claude_thinking_extensions(claude_extensions(
                block_object,
                &["type", "data"],
            )),
        }),
        "image" => claude_media_block_to_canonical(block_object, true),
        "document" => claude_media_block_to_canonical(block_object, false),
        "tool_use" => Some(CanonicalContentBlock::ToolUse {
            id: block_object
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: block_object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input: block_object
                .get("input")
                .cloned()
                .unwrap_or_else(|| json!({})),
            extensions: claude_extensions(block_object, &["type", "id", "name", "input"]),
        }),
        "tool_result" => {
            let content = block_object.get("content").cloned();
            let mut extensions = claude_extensions(
                block_object,
                &["type", "tool_use_id", "content", "is_error"],
            );
            extensions.insert(
                AETHER_EXTENSION_NAMESPACE.to_string(),
                json!({ "source": CLAUDE_TOOL_RESULT_SOURCE_MARKER }),
            );
            Some(CanonicalContentBlock::ToolResult {
                tool_use_id: block_object
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: None,
                output: content.clone(),
                content_text: content.as_ref().map(openai_responses_tool_output_text),
                is_error: block_object
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                extensions,
            })
        }
        _ => Some(CanonicalContentBlock::Unknown {
            raw_type,
            payload: block.clone(),
            extensions: claude_raw_extensions(BTreeMap::new()),
        }),
    }
}

pub(crate) fn claude_media_block_to_canonical(
    block: &Map<String, Value>,
    image: bool,
) -> Option<CanonicalContentBlock> {
    let source = block.get("source")?.as_object()?;
    let source_type = source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let media_type = source
        .get("media_type")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    match (image, source_type) {
        (true, "base64") => Some(CanonicalContentBlock::Image {
            data: source
                .get("data")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            url: None,
            media_type,
            detail: None,
            extensions: claude_extensions(block, &["type", "source"]),
        }),
        (true, "url") => Some(CanonicalContentBlock::Image {
            data: None,
            url: source
                .get("url")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            media_type: None,
            detail: None,
            extensions: claude_extensions(block, &["type", "source"]),
        }),
        (false, "base64")
            if media_type
                .as_deref()
                .is_some_and(|value| value.starts_with("audio/")) =>
        {
            let format = media_type
                .as_deref()
                .and_then(|value| value.strip_prefix("audio/"))
                .map(ToOwned::to_owned);
            Some(CanonicalContentBlock::Audio {
                data: source
                    .get("data")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                media_type,
                format,
                extensions: claude_extensions(block, &["type", "source"]),
            })
        }
        (false, "base64") => Some(CanonicalContentBlock::File {
            data: source
                .get("data")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            file_id: None,
            file_url: None,
            media_type,
            filename: None,
            extensions: claude_extensions(block, &["type", "source"]),
        }),
        (false, "url") => Some(CanonicalContentBlock::File {
            data: None,
            file_id: None,
            file_url: source
                .get("url")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            media_type: None,
            filename: None,
            extensions: claude_extensions(block, &["type", "source"]),
        }),
        _ => Some(CanonicalContentBlock::Unknown {
            raw_type: block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            payload: Value::Object(block.clone()),
            extensions: claude_raw_extensions(BTreeMap::new()),
        }),
    }
}

pub(crate) fn openai_message_content_blocks(
    message: &Map<String, Value>,
) -> Option<Vec<CanonicalContentBlock>> {
    let role = openai_role_to_canonical(message.get("role").and_then(Value::as_str).unwrap_or(""));
    let mut blocks = if role == CanonicalRole::Tool {
        Vec::new()
    } else {
        openai_content_to_blocks(message.get("content"))?
    };
    if role == CanonicalRole::Assistant {
        let reasoning_blocks = openai_reasoning_blocks(message);
        if !reasoning_blocks.is_empty() {
            blocks.splice(0..0, reasoning_blocks);
        } else if let Some(reasoning_content) = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let mut extensions = BTreeMap::new();
            canonical_extension_object_mut(&mut extensions, "openai")
                .insert("omit_reasoning_parts".to_string(), Value::Bool(true));
            let extensions = openai_thinking_extensions(extensions);
            blocks.insert(
                0,
                CanonicalContentBlock::Thinking {
                    text: reasoning_content.to_string(),
                    signature: None,
                    encrypted_content: None,
                    extensions,
                },
            );
        }
        if let Some(audio_object) = message.get("audio").and_then(Value::as_object) {
            blocks.push(openai_chat_audio_to_canonical_block(audio_object));
        }
    }
    let mut saw_tool_calls = false;
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let tool_call = tool_call.as_object()?;
            let tool_call_type = tool_call
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("function")
                .trim()
                .to_ascii_lowercase();
            saw_tool_calls = true;
            let id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if tool_call_type == "custom" {
                let custom = tool_call.get("custom").and_then(Value::as_object)?;
                let name = custom
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("custom_tool")
                    .to_string();
                let mut extensions = openai_extensions(tool_call, &["id", "type"]);
                mark_openai_custom_tool_call(&mut extensions);
                blocks.push(CanonicalContentBlock::ToolUse {
                    id,
                    name,
                    input: parse_jsonish_value(
                        custom.get("input").or_else(|| custom.get("arguments")),
                    ),
                    extensions,
                });
            } else {
                let function = tool_call.get("function").and_then(Value::as_object)?;
                blocks.push(CanonicalContentBlock::ToolUse {
                    id,
                    name: function
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: parse_jsonish_value(function.get("arguments")),
                    extensions: openai_extensions(tool_call, &["id", "type", "function"]),
                });
            }
        }
    }
    if role == CanonicalRole::Assistant && !saw_tool_calls {
        if let Some(function_call) = message.get("function_call").and_then(Value::as_object) {
            let name = function_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            blocks.push(CanonicalContentBlock::ToolUse {
                id: message
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(name.as_str())
                    .to_string(),
                name,
                input: parse_jsonish_value(function_call.get("arguments")),
                extensions: openai_extensions(message, &["role", "content", "function_call"]),
            });
        }
    }
    if role == CanonicalRole::Tool {
        let text = openai_content_text(message.get("content"));
        let mut extensions = openai_extensions(message, &["role", "content", "tool_call_id"]);
        extensions.insert(
            AETHER_EXTENSION_NAMESPACE.to_string(),
            json!({ "source": OPENAI_CHAT_TOOL_RESULT_SOURCE_MARKER }),
        );
        blocks.push(CanonicalContentBlock::ToolResult {
            tool_use_id: message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .or_else(|| message.get("name").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string(),
            name: None,
            output: match message.get("content") {
                Some(Value::String(raw)) => serde_json::from_str::<Value>(raw)
                    .ok()
                    .or_else(|| Some(Value::String(raw.clone()))),
                Some(value) => Some(value.clone()),
                None => None,
            },
            content_text: Some(if text.is_empty() {
                message
                    .get("content")
                    .map(openai_responses_tool_output_text)
                    .unwrap_or_default()
            } else {
                text
            }),
            is_error: false,
            extensions,
        });
    }
    Some(blocks)
}

fn openai_chat_audio_to_canonical_block(
    audio_object: &Map<String, Value>,
) -> CanonicalContentBlock {
    let mut extensions = BTreeMap::from([(
        "openai".to_string(),
        json!({ "audio": Value::Object(audio_object.clone()) }),
    )]);
    mark_openai_output_audio(&mut extensions);
    CanonicalContentBlock::Audio {
        data: audio_object
            .get("data")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        media_type: None,
        format: None,
        extensions,
    }
}

pub(crate) fn openai_reasoning_blocks(message: &Map<String, Value>) -> Vec<CanonicalContentBlock> {
    let Some(reasoning_parts) = message.get("reasoning_parts").and_then(Value::as_array) else {
        return Vec::new();
    };
    let omit_reasoning_content = message.get("reasoning_content").is_none();
    let mut blocks = Vec::new();
    for part in reasoning_parts {
        let Some(part_object) = part.as_object() else {
            continue;
        };
        match part_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("thinking")
        {
            "thinking" => {
                let text = part_object
                    .get("thinking")
                    .or_else(|| part_object.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                let mut extensions =
                    openai_extensions(part_object, &["type", "thinking", "text", "signature"]);
                if omit_reasoning_content {
                    canonical_extension_object_mut(&mut extensions, "openai")
                        .insert("omit_reasoning_content".to_string(), Value::Bool(true));
                }
                let extensions = openai_thinking_extensions(extensions);
                blocks.push(CanonicalContentBlock::Thinking {
                    text: text.to_string(),
                    signature: part_object
                        .get("signature")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    encrypted_content: None,
                    extensions,
                });
            }
            "redacted_thinking" => {
                if let Some(data) = part_object
                    .get("data")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    let extensions = openai_thinking_extensions(openai_extensions(
                        part_object,
                        &["type", "data"],
                    ));
                    blocks.push(CanonicalContentBlock::Thinking {
                        text: String::new(),
                        signature: None,
                        encrypted_content: Some(data.to_string()),
                        extensions,
                    });
                }
            }
            _ => {}
        }
    }
    blocks
}

pub(crate) fn openai_responses_input_to_canonical_messages(
    input: Option<&Value>,
) -> Option<Vec<CanonicalMessage>> {
    let Some(input) = input else {
        return Some(Vec::new());
    };
    match input {
        Value::Null => Some(Vec::new()),
        Value::String(text) => {
            if text.trim().is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![CanonicalMessage {
                    role: CanonicalRole::User,
                    content: vec![CanonicalContentBlock::Text {
                        text: text.clone(),
                        extensions: BTreeMap::new(),
                    }],
                    extensions: BTreeMap::new(),
                }])
            }
        }
        Value::Array(items) => {
            let mut messages = Vec::new();
            let mut next_generated_tool_call_index = 0usize;
            let mut pending_reasoning: Option<CanonicalContentBlock> = None;
            for item in items {
                if let Some(text) = item.as_str() {
                    if !text.trim().is_empty() {
                        messages.push(CanonicalMessage {
                            role: CanonicalRole::User,
                            content: vec![CanonicalContentBlock::Text {
                                text: text.to_string(),
                                extensions: BTreeMap::new(),
                            }],
                            extensions: BTreeMap::new(),
                        });
                    }
                    pending_reasoning = None;
                    continue;
                }
                let Some(item_object) = item.as_object() else {
                    messages.push(openai_responses_opaque_input_item_message(
                        item,
                        String::new(),
                    ));
                    pending_reasoning = None;
                    continue;
                };
                let item_type = item_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                    .trim()
                    .to_ascii_lowercase();
                match item_type.as_str() {
                    "reasoning" => {
                        pending_reasoning = openai_responses_reasoning_block_from_item(item_object);
                    }
                    "message" => {
                        let role = openai_role_to_canonical(
                            item_object
                                .get("role")
                                .and_then(Value::as_str)
                                .unwrap_or("user"),
                        );
                        let is_assistant = role == CanonicalRole::Assistant;
                        let mut extensions =
                            openai_responses_extensions(item_object, &["type", "role", "content"]);
                        mark_openai_responses_input_message(&mut extensions);
                        let mut message = CanonicalMessage {
                            role,
                            content: openai_responses_content_to_blocks(
                                item_object.get("content"),
                            )?,
                            extensions,
                        };
                        if is_assistant {
                            if let Some(reasoning) = pending_reasoning.take() {
                                prepend_openai_responses_reasoning_block(&mut message, reasoning);
                            }
                        } else {
                            pending_reasoning = None;
                        }
                        messages.push(message);
                    }
                    "function_call" => {
                        let name = item_object
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .unwrap_or_default()
                            .to_string();
                        let id = item_object
                            .get("call_id")
                            .or_else(|| item_object.get("id"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| {
                                let generated =
                                    format!("call_auto_{next_generated_tool_call_index}");
                                next_generated_tool_call_index += 1;
                                generated
                            });
                        let mut extensions = openai_responses_extensions(
                            item_object,
                            &["type", "call_id", "id", "name", "arguments"],
                        );
                        remember_openai_responses_tool_call_item_id(&mut extensions, item_object);
                        let tool_use = CanonicalContentBlock::ToolUse {
                            id,
                            name,
                            input: parse_jsonish_value(item_object.get("arguments")),
                            extensions,
                        };
                        append_openai_responses_tool_use(
                            &mut messages,
                            tool_use,
                            &mut pending_reasoning,
                        );
                    }
                    "custom_tool_call" => {
                        let name = item_object
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .unwrap_or("custom_tool")
                            .to_string();
                        let id = item_object
                            .get("call_id")
                            .or_else(|| item_object.get("id"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| {
                                let generated =
                                    format!("call_auto_{next_generated_tool_call_index}");
                                next_generated_tool_call_index += 1;
                                generated
                            });
                        let mut extensions = openai_responses_extensions(
                            item_object,
                            &[
                                "type",
                                "call_id",
                                "id",
                                "name",
                                "input",
                                "arguments",
                                "status",
                            ],
                        );
                        remember_openai_responses_tool_call_item_id(&mut extensions, item_object);
                        mark_openai_custom_tool_call(&mut extensions);
                        let tool_use = CanonicalContentBlock::ToolUse {
                            id,
                            name,
                            input: parse_jsonish_value(
                                item_object
                                    .get("input")
                                    .or_else(|| item_object.get("arguments")),
                            ),
                            extensions,
                        };
                        append_openai_responses_tool_use(
                            &mut messages,
                            tool_use,
                            &mut pending_reasoning,
                        );
                    }
                    "function_call_output" => {
                        let id = item_object
                            .get("call_id")
                            .or_else(|| item_object.get("tool_call_id"))
                            .or_else(|| item_object.get("id"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| {
                                let generated =
                                    format!("call_auto_{next_generated_tool_call_index}");
                                next_generated_tool_call_index += 1;
                                generated
                            });
                        let raw_output = item_object.get("output");
                        let output = Some(parse_jsonish_value(raw_output));
                        let mut extensions = openai_responses_extensions(
                            item_object,
                            &[
                                "type",
                                "call_id",
                                "tool_call_id",
                                "id",
                                "output",
                                "is_error",
                            ],
                        );
                        extensions.insert(
                            AETHER_EXTENSION_NAMESPACE.to_string(),
                            json!({ "source": OPENAI_RESPONSES_TOOL_RESULT_SOURCE_MARKER }),
                        );
                        messages.push(CanonicalMessage {
                            role: CanonicalRole::Tool,
                            content: vec![CanonicalContentBlock::ToolResult {
                                tool_use_id: id,
                                name: None,
                                content_text: raw_output.map(openai_responses_tool_output_text),
                                output,
                                is_error: item_object
                                    .get("is_error")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                                extensions,
                            }],
                            extensions: BTreeMap::new(),
                        });
                        pending_reasoning = None;
                    }
                    "custom_tool_call_output"
                    | "local_shell_call_output"
                    | "shell_call_output"
                    | "apply_patch_call_output"
                    | "computer_call_output" => {
                        messages.push(CanonicalMessage {
                            role: CanonicalRole::Tool,
                            content: vec![openai_responses_hosted_tool_result_to_block(
                                item_object,
                                item_type.as_str(),
                                next_generated_tool_call_index,
                            )],
                            extensions: BTreeMap::new(),
                        });
                        pending_reasoning = None;
                    }
                    _ => {
                        messages.push(openai_responses_opaque_input_item_message(item, item_type));
                        pending_reasoning = None;
                    }
                }
            }
            Some(messages)
        }
        Value::Object(_) => {
            openai_responses_input_to_canonical_messages(Some(&Value::Array(vec![input.clone()])))
        }
        _ => None,
    }
}

fn openai_responses_opaque_input_item_message(item: &Value, raw_type: String) -> CanonicalMessage {
    CanonicalMessage {
        role: CanonicalRole::Unknown,
        content: vec![CanonicalContentBlock::Unknown {
            raw_type,
            payload: item.clone(),
            extensions: openai_responses_raw_extensions(BTreeMap::new()),
        }],
        extensions: BTreeMap::new(),
    }
}

fn append_openai_responses_tool_use(
    messages: &mut Vec<CanonicalMessage>,
    tool_use: CanonicalContentBlock,
    pending_reasoning: &mut Option<CanonicalContentBlock>,
) {
    let reasoning = pending_reasoning.take();
    if let Some(last_message) = messages.last_mut() {
        if last_message.role == CanonicalRole::Assistant
            && (!is_openai_responses_input_message(&last_message.extensions)
                || canonical_assistant_message_has_visible_content(last_message))
        {
            if let Some(reasoning) = reasoning {
                prepend_openai_responses_reasoning_block(last_message, reasoning);
            }
            last_message.content.push(tool_use);
            return;
        }
    }

    let mut content = Vec::new();
    if let Some(reasoning) = reasoning {
        content.push(reasoning);
    }
    content.push(tool_use);
    messages.push(CanonicalMessage {
        role: CanonicalRole::Assistant,
        content,
        extensions: BTreeMap::new(),
    });
}

fn canonical_assistant_message_has_visible_content(message: &CanonicalMessage) -> bool {
    message.content.iter().any(|block| match block {
        CanonicalContentBlock::Text { text, .. } | CanonicalContentBlock::Thinking { text, .. } => {
            !text.trim().is_empty()
        }
        CanonicalContentBlock::Unknown { payload, .. } => !payload.is_null(),
        CanonicalContentBlock::Image { .. }
        | CanonicalContentBlock::File { .. }
        | CanonicalContentBlock::Audio { .. }
        | CanonicalContentBlock::ToolUse { .. }
        | CanonicalContentBlock::ToolResult { .. } => true,
    })
}

fn prepend_openai_responses_reasoning_block(
    message: &mut CanonicalMessage,
    reasoning: CanonicalContentBlock,
) {
    if message
        .content
        .iter()
        .any(|block| matches!(block, CanonicalContentBlock::Thinking { .. }))
    {
        return;
    }
    message.content.insert(0, reasoning);
}

fn openai_responses_reasoning_block_from_item(
    item_object: &Map<String, Value>,
) -> Option<CanonicalContentBlock> {
    let text = openai_responses_reasoning_text(item_object);
    let encrypted_content = item_object
        .get("encrypted_content")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if text.is_empty() && encrypted_content.is_none() {
        return None;
    }
    let mut extensions = BTreeMap::new();
    extensions.extend(openai_responses_extensions(
        item_object,
        &[
            "type",
            "id",
            "status",
            "summary",
            "content",
            "encrypted_content",
        ],
    ));
    canonical_extension_object_mut(&mut extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE).insert(
        "item_type".to_string(),
        Value::String("reasoning".to_string()),
    );
    canonical_extension_object_mut(&mut extensions, "openai")
        .insert("omit_reasoning_parts".to_string(), Value::Bool(true));
    let extensions = openai_thinking_extensions(extensions);
    Some(CanonicalContentBlock::Thinking {
        text,
        signature: None,
        encrypted_content,
        extensions,
    })
}

fn openai_responses_reasoning_text(item_object: &Map<String, Value>) -> String {
    let mut parts = openai_responses_reasoning_text_parts(item_object.get("summary"));
    if parts.is_empty() {
        parts = openai_responses_reasoning_text_parts(item_object.get("content"));
    }
    parts.join("\n")
}

fn openai_responses_reasoning_text_parts(raw: Option<&Value>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    match raw {
        Value::Array(items) => items
            .iter()
            .filter_map(openai_responses_reasoning_text_part)
            .collect(),
        other => openai_responses_reasoning_text_part(other)
            .into_iter()
            .collect(),
    }
}

fn openai_responses_reasoning_text_part(raw: &Value) -> Option<String> {
    if let Some(text) = raw.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }
    let raw_object = raw.as_object()?;
    let text = raw_object.get("text").and_then(Value::as_str)?;
    (!text.is_empty()).then(|| text.to_string())
}

pub(crate) fn openai_responses_content_to_blocks(
    content: Option<&Value>,
) -> Option<Vec<CanonicalContentBlock>> {
    let Some(content) = content else {
        return Some(Vec::new());
    };
    match content {
        Value::Null => Some(Vec::new()),
        Value::String(text) => Some(vec![CanonicalContentBlock::Text {
            text: text.clone(),
            extensions: BTreeMap::new(),
        }]),
        Value::Array(parts) => parts
            .iter()
            .map(openai_responses_part_to_canonical_block)
            .collect(),
        _ => None,
    }
}

pub(crate) fn openai_responses_output_to_canonical(
    output: Option<&Value>,
) -> Option<(Vec<CanonicalContentBlock>, BTreeMap<String, Value>)> {
    let Some(output) = output else {
        return Some((Vec::new(), BTreeMap::new()));
    };
    let output_items = output.as_array()?;
    let mut blocks = Vec::new();
    let mut message_item_provenance = Vec::new();
    for (index, item) in output_items.iter().enumerate() {
        let Some(item_object) = item.as_object() else {
            blocks.push(CanonicalContentBlock::Unknown {
                raw_type: String::new(),
                payload: item.clone(),
                extensions: openai_responses_raw_extensions(BTreeMap::new()),
            });
            continue;
        };
        let item_type = item_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match item_type.as_str() {
            "message" => {
                let message_extensions = openai_responses_extensions(
                    item_object,
                    &["type", "id", "status", "role", "content"],
                )
                .remove(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
                if !message_extensions.is_empty() {
                    message_item_provenance.push(json!({
                        "output_index": index,
                        "fields": message_extensions,
                    }));
                }
                blocks.extend(openai_responses_content_to_blocks(
                    item_object.get("content"),
                )?);
            }
            "reasoning" => {
                let mut emitted = false;
                let encrypted_content = item_object
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                if let Some(summary_items) = item_object.get("summary").and_then(Value::as_array) {
                    for summary in summary_items {
                        let Some(summary_object) = summary.as_object() else {
                            continue;
                        };
                        let text = summary_object
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if text.trim().is_empty() {
                            continue;
                        }
                        let mut extensions = openai_responses_extensions(
                            item_object,
                            &["type", "id", "status", "summary", "encrypted_content"],
                        );
                        canonical_extension_object_mut(&mut extensions, "openai")
                            .insert("omit_reasoning_parts".to_string(), Value::Bool(true));
                        let extensions = openai_thinking_extensions(extensions);
                        blocks.push(CanonicalContentBlock::Thinking {
                            text: text.to_string(),
                            signature: None,
                            encrypted_content: encrypted_content.clone(),
                            extensions,
                        });
                        emitted = true;
                    }
                }
                if !emitted && encrypted_content.is_some() {
                    let mut extensions = openai_responses_extensions(
                        item_object,
                        &["type", "id", "status", "summary", "encrypted_content"],
                    );
                    canonical_extension_object_mut(&mut extensions, "openai")
                        .insert("omit_reasoning_parts".to_string(), Value::Bool(true));
                    let extensions = openai_thinking_extensions(extensions);
                    blocks.push(CanonicalContentBlock::Thinking {
                        text: String::new(),
                        signature: None,
                        encrypted_content,
                        extensions,
                    });
                    emitted = true;
                }
                if !emitted {
                    blocks.push(CanonicalContentBlock::Unknown {
                        raw_type: item_type,
                        payload: item.clone(),
                        extensions: openai_responses_raw_extensions(BTreeMap::new()),
                    });
                }
            }
            "function_call" => {
                let name = item_object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())?;
                let id = item_object
                    .get("call_id")
                    .or_else(|| item_object.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("call_auto_{index}"));
                let mut extensions = openai_responses_extensions(
                    item_object,
                    &["type", "id", "call_id", "name", "arguments"],
                );
                remember_openai_responses_tool_call_item_id(&mut extensions, item_object);
                blocks.push(CanonicalContentBlock::ToolUse {
                    id,
                    name: name.to_string(),
                    input: parse_jsonish_value(item_object.get("arguments")),
                    extensions,
                });
            }
            "custom_tool_call" => {
                let name = item_object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("custom_tool");
                let id = item_object
                    .get("call_id")
                    .or_else(|| item_object.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("call_auto_{index}"));
                let mut extensions = openai_responses_extensions(
                    item_object,
                    &[
                        "type",
                        "id",
                        "call_id",
                        "name",
                        "input",
                        "arguments",
                        "status",
                    ],
                );
                remember_openai_responses_tool_call_item_id(&mut extensions, item_object);
                mark_openai_custom_tool_call(&mut extensions);
                blocks.push(CanonicalContentBlock::ToolUse {
                    id,
                    name: name.to_string(),
                    input: parse_jsonish_value(
                        item_object
                            .get("input")
                            .or_else(|| item_object.get("arguments")),
                    ),
                    extensions,
                });
            }
            "web_search_call" => {
                let id = item_object
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("call_auto_{index}"));
                let query = item_object
                    .get("action")
                    .and_then(Value::as_object)
                    .and_then(|action| action.get("query"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                blocks.push(CanonicalContentBlock::ToolUse {
                    id,
                    name: "web_search".to_string(),
                    input: json!({ "query": query }),
                    extensions: openai_responses_extensions(
                        item_object,
                        &["type", "id", "status", "action"],
                    ),
                });
            }
            "local_shell_call" | "shell_call" | "apply_patch_call" | "computer_call" => {
                blocks.push(openai_responses_hosted_tool_call_to_block(
                    item_object,
                    item_type.as_str(),
                    index,
                )?);
            }
            "function_call_output" => {
                let id = item_object
                    .get("call_id")
                    .or_else(|| item_object.get("tool_call_id"))
                    .or_else(|| item_object.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("call_auto_{index}"));
                let raw_output = item_object.get("output");
                let output = Some(parse_jsonish_value(raw_output));
                let mut extensions = openai_responses_extensions(
                    item_object,
                    &[
                        "type",
                        "id",
                        "call_id",
                        "tool_call_id",
                        "output",
                        "is_error",
                    ],
                );
                extensions.insert(
                    AETHER_EXTENSION_NAMESPACE.to_string(),
                    json!({ "source": OPENAI_RESPONSES_TOOL_RESULT_SOURCE_MARKER }),
                );
                blocks.push(CanonicalContentBlock::ToolResult {
                    tool_use_id: id,
                    name: None,
                    output,
                    content_text: raw_output.map(openai_responses_tool_output_text),
                    is_error: item_object
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    extensions,
                });
            }
            "custom_tool_call_output"
            | "local_shell_call_output"
            | "shell_call_output"
            | "apply_patch_call_output"
            | "computer_call_output" => {
                blocks.push(openai_responses_hosted_tool_result_to_block(
                    item_object,
                    item_type.as_str(),
                    index,
                ));
            }
            "image_generation_call" => {
                blocks.push(openai_responses_image_generation_call_to_block(
                    item_object,
                )?);
            }
            "output_text" | "text" | "output_image" | "image_url" | "file" | "input_file"
            | "input_audio" | "output_audio" => {
                blocks.push(openai_responses_part_to_canonical_block(item)?)
            }
            _ => blocks.push(CanonicalContentBlock::Unknown {
                raw_type: item_type,
                payload: item.clone(),
                extensions: openai_responses_raw_extensions(BTreeMap::new()),
            }),
        }
    }
    let mut extensions = BTreeMap::new();
    if !message_item_provenance.is_empty() {
        extensions.insert(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            json!({ "message_items": message_item_provenance }),
        );
    }
    Some((blocks, extensions))
}

fn openai_responses_hosted_tool_call_to_block(
    item_object: &Map<String, Value>,
    item_type: &str,
    index: usize,
) -> Option<CanonicalContentBlock> {
    let name = openai_responses_hosted_tool_name(item_type)?;
    let id = item_object
        .get("call_id")
        .or_else(|| item_object.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("call_auto_{index}"));
    let input = openai_responses_hosted_tool_input(item_object, item_type);
    let mut extensions = openai_responses_extensions(
        item_object,
        &[
            "type",
            "id",
            "call_id",
            "status",
            "action",
            "actions",
            "environment",
            "created_by",
            "max_output_length",
            "operation",
            "pending_safety_checks",
        ],
    );
    canonical_extension_object_mut(&mut extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE).insert(
        "item_type".to_string(),
        Value::String(item_type.to_string()),
    );
    Some(CanonicalContentBlock::ToolUse {
        id,
        name: name.to_string(),
        input,
        extensions,
    })
}

fn openai_responses_hosted_tool_result_to_block(
    item_object: &Map<String, Value>,
    item_type: &str,
    index: usize,
) -> CanonicalContentBlock {
    let id = item_object
        .get("call_id")
        .or_else(|| item_object.get("tool_call_id"))
        .or_else(|| item_object.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("call_auto_{index}"));
    let raw_output = item_object
        .get("output")
        .or_else(|| item_object.get("content"));
    let mut extensions = openai_responses_extensions(
        item_object,
        &[
            "type",
            "id",
            "call_id",
            "tool_call_id",
            "output",
            "content",
            "is_error",
        ],
    );
    extensions.insert(
        AETHER_EXTENSION_NAMESPACE.to_string(),
        json!({ "source": OPENAI_RESPONSES_TOOL_RESULT_SOURCE_MARKER }),
    );
    canonical_extension_object_mut(&mut extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE).insert(
        "item_type".to_string(),
        Value::String(item_type.to_string()),
    );
    CanonicalContentBlock::ToolResult {
        tool_use_id: id,
        name: openai_responses_hosted_tool_result_name(item_type).map(ToOwned::to_owned),
        output: raw_output.map(|_| parse_jsonish_value(raw_output)),
        content_text: raw_output.map(openai_responses_tool_output_text),
        is_error: item_object
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        extensions,
    }
}

fn openai_responses_hosted_tool_name(item_type: &str) -> Option<&'static str> {
    match item_type {
        "local_shell_call" => Some("local_shell"),
        "shell_call" => Some("shell"),
        "apply_patch_call" => Some("apply_patch"),
        "computer_call" => Some("computer"),
        _ => None,
    }
}

fn openai_responses_hosted_tool_result_name(item_type: &str) -> Option<&'static str> {
    match item_type {
        "local_shell_call_output" => Some("local_shell"),
        "shell_call_output" => Some("shell"),
        "apply_patch_call_output" => Some("apply_patch"),
        "computer_call_output" => Some("computer"),
        _ => None,
    }
}

fn openai_responses_hosted_tool_input(item_object: &Map<String, Value>, item_type: &str) -> Value {
    let fields = match item_type {
        "local_shell_call" | "shell_call" => &[
            "action",
            "environment",
            "status",
            "created_by",
            "max_output_length",
        ][..],
        "apply_patch_call" => &["operation", "status"][..],
        "computer_call" => &["action", "actions", "pending_safety_checks", "status"][..],
        _ => &[][..],
    };
    let mut input = Map::new();
    for field in fields {
        if let Some(value) = item_object.get(*field) {
            input.insert((*field).to_string(), value.clone());
        }
    }
    Value::Object(input)
}

fn openai_responses_image_generation_call_to_block(
    item_object: &Map<String, Value>,
) -> Option<CanonicalContentBlock> {
    let result = item_object
        .get("result")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let url = item_object
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let raw_image = result.or(url)?;
    let fallback_media_type = item_object
        .get("mime_type")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            item_object
                .get("output_format")
                .and_then(Value::as_str)
                .map(openai_responses_output_format_to_mime_type)
        });
    let (media_type, data, url) = if raw_image.starts_with("data:image/") {
        split_data_url(Some(raw_image.to_string()), fallback_media_type)
    } else if raw_image.starts_with("http://") || raw_image.starts_with("https://") {
        (fallback_media_type, None, Some(raw_image.to_string()))
    } else if result.is_some() {
        (
            fallback_media_type.or_else(|| Some("image/png".to_string())),
            Some(raw_image.to_string()),
            None,
        )
    } else {
        (fallback_media_type, None, Some(raw_image.to_string()))
    };
    let mut extensions = openai_responses_extensions(
        item_object,
        &[
            "type",
            "id",
            "status",
            "action",
            "result",
            "url",
            "output_format",
            "mime_type",
        ],
    );
    canonical_extension_object_mut(&mut extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE).insert(
        "item_type".to_string(),
        Value::String("image_generation_call".to_string()),
    );
    Some(CanonicalContentBlock::Image {
        data,
        url,
        media_type,
        detail: None,
        extensions,
    })
}

fn openai_responses_output_format_to_mime_type(output_format: &str) -> String {
    match output_format.trim().to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    }
    .to_string()
}

pub(crate) fn openai_responses_part_to_canonical_block(
    part: &Value,
) -> Option<CanonicalContentBlock> {
    let part_object = part.as_object()?;
    let raw_type = part_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match raw_type.as_str() {
        "input_text" | "output_text" | "text" => Some(CanonicalContentBlock::Text {
            text: part_object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            extensions: {
                let mut extensions = openai_responses_extensions(part_object, &["type", "text"]);
                mark_openai_responses_content_block(&mut extensions);
                extensions
            },
        }),
        "reasoning" | "thinking" => Some(CanonicalContentBlock::Thinking {
            text: part_object
                .get("text")
                .or_else(|| part_object.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            signature: part_object
                .get("signature")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            encrypted_content: part_object
                .get("encrypted_content")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            extensions: {
                let mut extensions = openai_responses_extensions(
                    part_object,
                    &["type", "text", "summary", "signature", "encrypted_content"],
                );
                canonical_extension_object_mut(&mut extensions, "openai")
                    .insert("omit_reasoning_parts".to_string(), Value::Bool(true));
                openai_thinking_extensions(extensions)
            },
        }),
        "input_image" | "output_image" | "image_url" => {
            let image_object = part_object.get("image_url").and_then(Value::as_object);
            let url = part_object
                .get("image_url")
                .and_then(Value::as_str)
                .or_else(|| {
                    image_object
                        .and_then(|image| image.get("url"))
                        .and_then(Value::as_str)
                })
                .or_else(|| part_object.get("url").and_then(Value::as_str))
                .map(ToOwned::to_owned);
            let detail = part_object
                .get("detail")
                .and_then(Value::as_str)
                .or_else(|| {
                    image_object
                        .and_then(|image| image.get("detail"))
                        .and_then(Value::as_str)
                })
                .map(ToOwned::to_owned);
            let (media_type, data, url) = split_data_url(url, None);
            Some(CanonicalContentBlock::Image {
                data,
                url,
                media_type,
                detail,
                extensions: openai_responses_extensions(
                    part_object,
                    &["type", "image_url", "url", "detail"],
                ),
            })
        }
        "input_file" | "file" => {
            let file_object = part_object
                .get("file")
                .and_then(Value::as_object)
                .unwrap_or(part_object);
            let file_data = file_object
                .get("file_data")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let (media_type, data, file_url) = split_data_url(
                file_data.or_else(|| {
                    file_object
                        .get("file_url")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                }),
                file_object
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            );
            Some(CanonicalContentBlock::File {
                data,
                file_id: file_object
                    .get("file_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                file_url,
                media_type,
                filename: file_object
                    .get("filename")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                extensions: openai_responses_extensions(
                    part_object,
                    &[
                        "type",
                        "file_data",
                        "file_url",
                        "mime_type",
                        "file_id",
                        "filename",
                    ],
                ),
            })
        }
        "input_audio" | "audio" => {
            let audio_object = part_object
                .get("input_audio")
                .and_then(Value::as_object)
                .unwrap_or(part_object);
            let format = audio_object
                .get("format")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            Some(CanonicalContentBlock::Audio {
                data: audio_object
                    .get("data")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                media_type: audio_object
                    .get("media_type")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| format.as_ref().map(|value| format!("audio/{value}"))),
                format,
                extensions: openai_responses_extensions(part_object, &["type", "input_audio"]),
            })
        }
        "output_audio" => {
            let mut extensions = openai_responses_extensions(part_object, &["type", "data"]);
            canonical_extension_object_mut(&mut extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE)
                .insert(
                    "item_type".to_string(),
                    Value::String("output_audio".to_string()),
                );
            mark_openai_output_audio(&mut extensions);
            Some(CanonicalContentBlock::Audio {
                data: part_object
                    .get("data")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                media_type: None,
                format: None,
                extensions,
            })
        }
        "refusal" => Some(CanonicalContentBlock::Unknown {
            raw_type,
            payload: part.clone(),
            extensions: openai_responses_raw_content_extensions(BTreeMap::new()),
        }),
        _ => Some(CanonicalContentBlock::Unknown {
            raw_type,
            payload: part.clone(),
            extensions: openai_responses_raw_content_extensions(BTreeMap::new()),
        }),
    }
}

pub(crate) fn openai_content_to_blocks(
    content: Option<&Value>,
) -> Option<Vec<CanonicalContentBlock>> {
    let Some(content) = content else {
        return Some(Vec::new());
    };
    match content {
        Value::Null => Some(Vec::new()),
        Value::String(text) => Some(vec![CanonicalContentBlock::Text {
            text: text.clone(),
            extensions: BTreeMap::new(),
        }]),
        Value::Array(parts) => parts.iter().map(openai_part_to_canonical_block).collect(),
        _ => None,
    }
}

pub(crate) fn openai_part_to_canonical_block(part: &Value) -> Option<CanonicalContentBlock> {
    let part_object = part.as_object()?;
    let raw_type = part_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match raw_type.as_str() {
        "text" | "input_text" | "output_text" => Some(CanonicalContentBlock::Text {
            text: part_object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            extensions: openai_extensions(part_object, &["type", "text"]),
        }),
        "image_url" | "input_image" | "output_image" => {
            let image_object = part_object.get("image_url").and_then(Value::as_object);
            let url = part_object
                .get("image_url")
                .and_then(Value::as_str)
                .or_else(|| {
                    image_object
                        .and_then(|image| image.get("url"))
                        .and_then(Value::as_str)
                })
                .or_else(|| part_object.get("url").and_then(Value::as_str))
                .map(ToOwned::to_owned);
            let detail = part_object
                .get("detail")
                .and_then(Value::as_str)
                .or_else(|| {
                    image_object
                        .and_then(|image| image.get("detail"))
                        .and_then(Value::as_str)
                })
                .map(ToOwned::to_owned);
            let (media_type, data, url) = split_data_url(url, None);
            Some(CanonicalContentBlock::Image {
                data,
                url,
                media_type,
                detail,
                extensions: openai_extensions(part_object, &["type", "image_url", "url", "detail"]),
            })
        }
        "file" | "input_file" => {
            let file_object = part_object
                .get("file")
                .and_then(Value::as_object)
                .unwrap_or(part_object);
            let file_data = file_object
                .get("file_data")
                .and_then(Value::as_str)
                .map(str::to_string);
            let (media_type, data, file_url) = split_data_url(
                file_data.or_else(|| {
                    file_object
                        .get("file_url")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                }),
                file_object
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
            Some(CanonicalContentBlock::File {
                data,
                file_id: file_object
                    .get("file_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                file_url,
                media_type,
                filename: file_object
                    .get("filename")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                extensions: openai_extensions(part_object, &["type", "file"]),
            })
        }
        "input_audio" => {
            let audio_object = part_object.get("input_audio").and_then(Value::as_object)?;
            let data = audio_object
                .get("data")
                .and_then(Value::as_str)
                .map(str::to_string);
            let format = audio_object
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(CanonicalContentBlock::Audio {
                data,
                media_type: format.as_ref().map(|value| format!("audio/{value}")),
                format,
                extensions: openai_extensions(part_object, &["type", "input_audio"]),
            })
        }
        _ => Some(CanonicalContentBlock::Unknown {
            raw_type,
            payload: part.clone(),
            extensions: BTreeMap::new(),
        }),
    }
}

pub(crate) fn canonical_message_to_openai_chat_messages(message: &CanonicalMessage) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut pending_start = 0usize;
    let mut saw_tool_result = false;

    for (index, block) in message.content.iter().enumerate() {
        if let CanonicalContentBlock::ToolResult { .. } = block {
            saw_tool_result = true;
            if pending_start < index {
                if let Some(message_value) = canonical_message_blocks_to_openai_chat(
                    message,
                    &message.content[pending_start..index],
                    false,
                ) {
                    messages.push(message_value);
                }
            }
            messages.push(canonical_tool_result_to_openai_chat(block));
            pending_start = index + 1;
        }
    }

    if !saw_tool_result {
        return vec![canonical_message_without_tool_results_to_openai_chat(
            message,
        )];
    }

    if pending_start < message.content.len() {
        if let Some(message_value) = canonical_message_blocks_to_openai_chat(
            message,
            &message.content[pending_start..],
            false,
        ) {
            messages.push(message_value);
        }
    }

    messages
}

fn canonical_message_without_tool_results_to_openai_chat(message: &CanonicalMessage) -> Value {
    debug_assert!(
        !message
            .content
            .iter()
            .any(|block| matches!(block, CanonicalContentBlock::ToolResult { .. })),
        "single OpenAI Chat message emission requires no ToolResult blocks; use canonical_message_to_openai_chat_messages"
    );
    canonical_message_blocks_to_openai_chat(message, &message.content, true)
        .expect("include_empty=true always emits a chat message")
}

fn canonical_message_blocks_to_openai_chat(
    message: &CanonicalMessage,
    content: &[CanonicalContentBlock],
    include_empty: bool,
) -> Option<Value> {
    let mut output = Map::new();
    output.insert(
        "role".to_string(),
        Value::String(
            match message.role {
                CanonicalRole::Assistant => "assistant",
                CanonicalRole::System => "system",
                CanonicalRole::Developer => "system",
                CanonicalRole::Tool => "tool",
                CanonicalRole::Unknown | CanonicalRole::User => "user",
            }
            .to_string(),
        ),
    );
    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_segments = Vec::new();
    let mut reasoning_parts = Vec::new();
    for block in content {
        match block {
            CanonicalContentBlock::Thinking {
                text,
                signature,
                encrypted_content,
                extensions,
            } if matches!(message.role, CanonicalRole::Assistant) => {
                if let Some(data) = encrypted_content.as_ref().filter(|value| !value.is_empty()) {
                    reasoning_parts.push(json!({
                        "type": "redacted_thinking",
                        "data": data,
                    }));
                    continue;
                }
                if !text.trim().is_empty() {
                    let omit_reasoning_content = extensions
                        .get("openai")
                        .and_then(|value| value.get("omit_reasoning_content"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let omit_reasoning_parts = extensions
                        .get("openai")
                        .and_then(|value| value.get("omit_reasoning_parts"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !omit_reasoning_content {
                        reasoning_segments.push(text.clone());
                    }
                    if !omit_reasoning_parts {
                        let mut reasoning_part = Map::new();
                        reasoning_part
                            .insert("type".to_string(), Value::String("thinking".to_string()));
                        reasoning_part.insert("thinking".to_string(), Value::String(text.clone()));
                        if let Some(signature) =
                            signature.as_ref().filter(|value| !value.is_empty())
                        {
                            reasoning_part
                                .insert("signature".to_string(), Value::String(signature.clone()));
                        }
                        reasoning_parts.push(Value::Object(reasoning_part));
                    }
                }
            }
            CanonicalContentBlock::ToolUse {
                id,
                name,
                input,
                extensions,
            } => tool_calls.push(canonical_tool_use_to_openai_chat_tool_call(
                id, name, input, extensions,
            )),
            CanonicalContentBlock::ToolResult { .. } => {}
            other => {
                if let Some(part) = canonical_content_block_to_openai_part(other) {
                    content_parts.push(part);
                }
            }
        }
    }
    if !include_empty
        && content_parts.is_empty()
        && tool_calls.is_empty()
        && reasoning_segments.is_empty()
        && reasoning_parts.is_empty()
    {
        return None;
    }
    output.insert(
        "content".to_string(),
        if !tool_calls.is_empty() && content_parts.is_empty() {
            Value::Null
        } else {
            openai_content_value_from_parts(content_parts, false)
        },
    );
    if !tool_calls.is_empty() {
        output.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    if !reasoning_segments.is_empty() {
        output.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_segments.join("")),
        );
    }
    if !reasoning_parts.is_empty() {
        output.insert("reasoning_parts".to_string(), Value::Array(reasoning_parts));
    }
    Some(Value::Object(output))
}

fn canonical_tool_result_to_openai_chat(block: &CanonicalContentBlock) -> Value {
    let CanonicalContentBlock::ToolResult {
        tool_use_id,
        content_text,
        output: result_output,
        is_error,
        extensions,
        ..
    } = block
    else {
        unreachable!("canonical_tool_result_to_openai_chat requires ToolResult");
    };

    let mut output = Map::new();
    output.insert("role".to_string(), Value::String("tool".to_string()));
    output.insert(
        "tool_call_id".to_string(),
        Value::String(tool_use_id.clone()),
    );
    let content = if is_claude_tool_result(extensions) {
        let content =
            openai_chat_tool_result_content(result_output.as_ref(), content_text.as_deref());
        if *is_error {
            openai_chat_tool_error_content(content)
        } else {
            content
        }
    } else if is_openai_responses_tool_result(extensions) {
        openai_responses_tool_result_content_for_chat(
            result_output.as_ref(),
            content_text.as_deref(),
        )
    } else {
        result_output
            .clone()
            .unwrap_or_else(|| Value::String(content_text.clone().unwrap_or_default()))
    };
    output.insert("content".to_string(), content);
    Value::Object(output)
}

fn claude_thinking_extensions(mut extensions: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    canonical_extension_object_mut(&mut extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(CLAUDE_THINKING_SOURCE_MARKER.to_string()),
    );
    extensions
}

fn claude_raw_extensions(mut extensions: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    canonical_extension_object_mut(&mut extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(CLAUDE_RAW_SOURCE_MARKER.to_string()),
    );
    extensions
}

fn openai_responses_raw_extensions(
    mut extensions: BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    canonical_extension_object_mut(&mut extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(OPENAI_RESPONSES_RAW_SOURCE_MARKER.to_string()),
    );
    extensions
}

fn openai_responses_raw_content_extensions(
    mut extensions: BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    canonical_extension_object_mut(&mut extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(OPENAI_RESPONSES_RAW_CONTENT_SOURCE_MARKER.to_string()),
    );
    extensions
}

fn mark_openai_responses_content_block(extensions: &mut BTreeMap<String, Value>) {
    canonical_extension_object_mut(extensions, AETHER_EXTENSION_NAMESPACE).insert(
        OPENAI_RESPONSES_CONTENT_MARKER.to_string(),
        Value::Bool(true),
    );
}

fn openai_thinking_extensions(mut extensions: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    canonical_extension_object_mut(&mut extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(OPENAI_THINKING_SOURCE_MARKER.to_string()),
    );
    extensions
}

fn mark_openai_custom_tool_call(extensions: &mut BTreeMap<String, Value>) {
    canonical_extension_object_mut(extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(OPENAI_CUSTOM_TOOL_CALL_SOURCE_MARKER.to_string()),
    );
}

fn remember_openai_responses_tool_call_item_id(
    extensions: &mut BTreeMap<String, Value>,
    item_object: &Map<String, Value>,
) {
    if let Some(item_id) = item_object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        canonical_extension_object_mut(extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE)
            .insert("item_id".to_string(), Value::String(item_id.to_string()));
    }
}

fn mark_openai_output_audio(extensions: &mut BTreeMap<String, Value>) {
    canonical_extension_object_mut(extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(OPENAI_OUTPUT_AUDIO_SOURCE_MARKER.to_string()),
    );
}

fn mark_openai_responses_input_message(extensions: &mut BTreeMap<String, Value>) {
    canonical_extension_object_mut(extensions, AETHER_EXTENSION_NAMESPACE).insert(
        "source".to_string(),
        Value::String(OPENAI_RESPONSES_INPUT_MESSAGE_SOURCE_MARKER.to_string()),
    );
}

pub(crate) fn is_claude_thinking_block(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(CLAUDE_THINKING_SOURCE_MARKER)
}

fn is_claude_raw_block(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(CLAUDE_RAW_SOURCE_MARKER)
}

pub(crate) fn is_openai_responses_raw_block(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_RESPONSES_RAW_SOURCE_MARKER)
}

pub(crate) fn is_openai_responses_raw_content_block(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_RESPONSES_RAW_CONTENT_SOURCE_MARKER)
}

pub(crate) fn is_openai_responses_content_block(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get(OPENAI_RESPONSES_CONTENT_MARKER))
        .and_then(Value::as_bool)
        == Some(true)
}

pub(crate) fn is_openai_responses_input_message(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_RESPONSES_INPUT_MESSAGE_SOURCE_MARKER)
}

pub(crate) fn is_openai_thinking_block(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_THINKING_SOURCE_MARKER)
}

pub(crate) fn is_openai_custom_tool_call(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_CUSTOM_TOOL_CALL_SOURCE_MARKER)
}

fn is_openai_output_audio_block(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_OUTPUT_AUDIO_SOURCE_MARKER)
}

pub(crate) fn is_claude_tool_result(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(CLAUDE_TOOL_RESULT_SOURCE_MARKER)
}

fn is_openai_responses_tool_result(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_RESPONSES_TOOL_RESULT_SOURCE_MARKER)
}

fn openai_responses_tool_result_content_for_chat(
    output: Option<&Value>,
    content_text: Option<&str>,
) -> Value {
    if let Some(text) = content_text {
        return Value::String(text.to_string());
    }
    match output {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(value) => Value::String(value.to_string()),
        None => Value::String(String::new()),
    }
}

fn openai_chat_tool_result_content(output: Option<&Value>, content_text: Option<&str>) -> Value {
    match output {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(Value::Array(parts)) => anthropic_tool_result_blocks_to_openai_chat_content(parts),
        Some(value) => Value::String(value.to_string()),
        None => Value::String(content_text.unwrap_or_default().to_string()),
    }
}

fn openai_chat_tool_error_content(content: Value) -> Value {
    match content {
        Value::String(text) if text.is_empty() => {
            Value::String(OPENAI_CHAT_TOOL_ERROR_PREFIX.to_string())
        }
        Value::String(text) => Value::String(format!("{OPENAI_CHAT_TOOL_ERROR_PREFIX}\n{text}")),
        Value::Array(parts) => {
            let mut prefixed_parts = Vec::with_capacity(parts.len() + 1);
            prefixed_parts.push(openai_text_part(OPENAI_CHAT_TOOL_ERROR_PREFIX));
            prefixed_parts.extend(parts);
            Value::Array(prefixed_parts)
        }
        value => Value::String(format!("{OPENAI_CHAT_TOOL_ERROR_PREFIX}\n{value}")),
    }
}

fn anthropic_tool_result_blocks_to_openai_chat_content(parts: &[Value]) -> Value {
    if let Some(text) = anthropic_text_blocks_to_string(parts) {
        return Value::String(text);
    }

    let mut has_media_part = false;
    let mut converted_parts = Vec::with_capacity(parts.len());
    for part in parts {
        let Some(openai_part) = anthropic_tool_result_block_to_openai_chat_part(part) else {
            return Value::String(Value::Array(parts.to_vec()).to_string());
        };
        if !openai_chat_part_is_text(&openai_part) {
            has_media_part = true;
        }
        converted_parts.push(openai_part);
    }

    if has_media_part {
        Value::Array(converted_parts)
    } else {
        Value::String(openai_text_parts_to_string(&converted_parts))
    }
}

fn anthropic_text_blocks_to_string(parts: &[Value]) -> Option<String> {
    let mut texts = Vec::with_capacity(parts.len());
    for part in parts {
        let part_object = part.as_object()?;
        if part_object.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        texts.push(part_object.get("text").and_then(Value::as_str)?);
    }
    Some(texts.join("\n\n"))
}

fn anthropic_tool_result_block_to_openai_chat_part(part: &Value) -> Option<Value> {
    let part_object = part.as_object()?;
    match part_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => Some(openai_text_part(
            part_object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )),
        "image" => anthropic_image_block_to_openai_chat_part(part_object),
        "document" | "file" => anthropic_document_block_to_openai_chat_part(part_object),
        _ => None,
    }
}

fn anthropic_image_block_to_openai_chat_part(block: &Map<String, Value>) -> Option<Value> {
    let source = block.get("source")?.as_object()?;
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => {
            let media_type = anthropic_source_media_type(source)?;
            let data = anthropic_source_str(source, "data")?;
            Some(json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{media_type};base64,{data}"),
                },
            }))
        }
        "url" => {
            let url = anthropic_source_str(source, "url")?;
            Some(json!({
                "type": "image_url",
                "image_url": {
                    "url": url,
                },
            }))
        }
        _ => None,
    }
}

fn anthropic_document_block_to_openai_chat_part(block: &Map<String, Value>) -> Option<Value> {
    let source = block.get("source")?.as_object()?;
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => {
            let media_type = anthropic_source_media_type(source)?;
            let data = anthropic_source_str(source, "data")?;
            Some(json!({
                "type": "file",
                "file": {
                    "file_data": format!("data:{media_type};base64,{data}"),
                },
            }))
        }
        "url" => {
            let url = anthropic_source_str(source, "url")?;
            Some(openai_text_part(format!("[File: {url}]")))
        }
        "text" => anthropic_source_str(source, "data").map(openai_text_part),
        _ => None,
    }
}

fn openai_text_part(text: impl Into<String>) -> Value {
    json!({
        "type": "text",
        "text": text.into(),
    })
}

fn openai_chat_part_is_text(part: &Value) -> bool {
    part.as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        == Some("text")
}

fn openai_text_parts_to_string(parts: &[Value]) -> String {
    parts
        .iter()
        .filter_map(|part| {
            part.as_object()
                .and_then(|object| object.get("text"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn anthropic_source_media_type(source: &Map<String, Value>) -> Option<&str> {
    source
        .get("media_type")
        .or_else(|| source.get("mime_type"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn anthropic_source_str<'a>(source: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    source
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

pub(crate) fn canonical_content_block_to_openai_part(
    block: &CanonicalContentBlock,
) -> Option<Value> {
    match block {
        CanonicalContentBlock::Text { text, extensions } => {
            let mut part = Map::new();
            part.insert("type".to_string(), Value::String("text".to_string()));
            part.insert("text".to_string(), Value::String(text.clone()));
            insert_openai_prompt_cache_breakpoint(&mut part, extensions);
            Some(Value::Object(part))
        }
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            detail,
            extensions,
        } => {
            let mut image = Map::new();
            image.insert(
                "url".to_string(),
                Value::String(media_data_or_url(media_type, data, url)),
            );
            if let Some(detail) = detail {
                image.insert("detail".to_string(), Value::String(detail.clone()));
            }
            let mut part = Map::new();
            part.insert("type".to_string(), Value::String("image_url".to_string()));
            part.insert("image_url".to_string(), Value::Object(image));
            insert_openai_prompt_cache_breakpoint(&mut part, extensions);
            Some(Value::Object(part))
        }
        CanonicalContentBlock::File {
            data,
            file_id,
            file_url,
            media_type,
            filename,
            extensions,
        } => {
            let mut file = Map::new();
            if let Some(value) = file_id {
                file.insert("file_id".to_string(), Value::String(value.clone()));
            }
            if data.is_some() || file_url.is_some() {
                if data.is_some() {
                    file.insert(
                        "file_data".to_string(),
                        Value::String(media_data_or_url(media_type, data, file_url)),
                    );
                } else if let Some(value) = file_url {
                    file.insert("file_url".to_string(), Value::String(value.clone()));
                }
            }
            if let Some(value) = filename {
                file.insert("filename".to_string(), Value::String(value.clone()));
            }
            let mut part = Map::new();
            part.insert("type".to_string(), Value::String("file".to_string()));
            part.insert("file".to_string(), Value::Object(file));
            insert_openai_prompt_cache_breakpoint(&mut part, extensions);
            Some(Value::Object(part))
        }
        CanonicalContentBlock::Audio {
            data,
            format,
            extensions,
            ..
        } => {
            let mut part = Map::new();
            part.insert("type".to_string(), Value::String("input_audio".to_string()));
            part.insert(
                "input_audio".to_string(),
                json!({
                    "data": data.clone().unwrap_or_default(),
                    "format": format.clone().unwrap_or_else(|| "mp3".to_string()),
                }),
            );
            insert_openai_prompt_cache_breakpoint(&mut part, extensions);
            Some(Value::Object(part))
        }
        CanonicalContentBlock::Thinking { text, .. } => Some(json!({
            "type": "text",
            "text": text,
        })),
        CanonicalContentBlock::ToolUse { .. }
        | CanonicalContentBlock::ToolResult { .. }
        | CanonicalContentBlock::Unknown { .. } => None,
    }
}

pub(crate) fn openai_prompt_cache_breakpoint_from_extensions(
    extensions: &BTreeMap<String, Value>,
) -> Option<Value> {
    [
        "openai",
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    ]
    .into_iter()
    .find_map(|namespace| {
        extensions
            .get(namespace)
            .and_then(|value| value.get("prompt_cache_breakpoint"))
            .cloned()
    })
}

fn insert_openai_prompt_cache_breakpoint(
    part: &mut Map<String, Value>,
    extensions: &BTreeMap<String, Value>,
) {
    if let Some(value) = openai_prompt_cache_breakpoint_from_extensions(extensions) {
        part.insert("prompt_cache_breakpoint".to_string(), value);
    }
}

pub(crate) fn canonical_content_block_to_openai_responses_part(
    block: &CanonicalContentBlock,
) -> Option<Value> {
    match block {
        CanonicalContentBlock::Text { text, extensions } => {
            let mut part = json!({
                "type": "output_text",
                "text": text,
                "annotations": [],
            });
            if let Some(annotations) = openai_responses_extension(extensions)
                .and_then(|value| value.get("annotations"))
                .cloned()
            {
                part["annotations"] = annotations;
            }
            Some(part)
        }
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            detail,
            ..
        } => {
            let mut part = Map::new();
            part.insert(
                "type".to_string(),
                Value::String("output_image".to_string()),
            );
            part.insert(
                "image_url".to_string(),
                Value::String(media_data_or_url(media_type, data, url)),
            );
            if let Some(detail) = detail {
                part.insert("detail".to_string(), Value::String(detail.clone()));
            }
            Some(Value::Object(part))
        }
        CanonicalContentBlock::File {
            data,
            file_id,
            file_url,
            media_type,
            filename,
            ..
        } => {
            let mut file = Map::new();
            if let Some(value) = file_id {
                file.insert("file_id".to_string(), Value::String(value.clone()));
            }
            if data.is_some() || file_url.is_some() {
                if data.is_some() {
                    file.insert(
                        "file_data".to_string(),
                        Value::String(media_data_or_url(media_type, data, file_url)),
                    );
                } else if let Some(value) = file_url {
                    file.insert("file_url".to_string(), Value::String(value.clone()));
                }
            }
            if let Some(value) = filename {
                file.insert("filename".to_string(), Value::String(value.clone()));
            }
            Some(json!({
                "type": "file",
                "file": Value::Object(file),
            }))
        }
        CanonicalContentBlock::Audio {
            data,
            format,
            extensions,
            ..
        } => {
            if is_openai_output_audio_block(extensions) {
                let mut part = Map::new();
                part.insert(
                    "type".to_string(),
                    Value::String("output_audio".to_string()),
                );
                if let Some(data) = data.as_ref().filter(|value| !value.is_empty()) {
                    part.insert("data".to_string(), Value::String(data.clone()));
                } else if let Some(raw_data) = openai_responses_extension(extensions)
                    .and_then(|value| value.get("data"))
                    .cloned()
                {
                    part.insert("data".to_string(), raw_data);
                }
                if let Some(transcript) = openai_responses_extension(extensions)
                    .and_then(|value| value.get("transcript"))
                    .cloned()
                    .or_else(|| {
                        extensions
                            .get("openai")
                            .and_then(|value| value.get("audio"))
                            .and_then(|value| value.get("transcript"))
                            .cloned()
                    })
                {
                    part.insert("transcript".to_string(), transcript);
                }
                return Some(Value::Object(part));
            }
            Some(json!({
                "type": "input_audio",
                "input_audio": {
                    "data": data.clone().unwrap_or_default(),
                    "format": format.clone().unwrap_or_else(|| "mp3".to_string()),
                }
            }))
        }
        CanonicalContentBlock::Thinking { .. }
        | CanonicalContentBlock::ToolUse { .. }
        | CanonicalContentBlock::ToolResult { .. }
        | CanonicalContentBlock::Unknown { .. } => None,
    }
}

pub(crate) fn flush_openai_responses_message_item(
    output: &mut Vec<Value>,
    message_content: &mut Vec<Value>,
    response_id: &str,
    message_index: &mut usize,
) {
    if message_content.is_empty() {
        return;
    }
    let id = if *message_index == 0 {
        format!("{response_id}_msg")
    } else {
        format!("{response_id}_msg_{message_index}")
    };
    output.push(json!({
        "type": "message",
        "id": id,
        "role": "assistant",
        "status": "completed",
        "content": coalesce_openai_responses_text_content(std::mem::take(message_content)),
    }));
    *message_index += 1;
}

pub(crate) fn openai_content_value_from_parts(parts: Vec<Value>, tool_only: bool) -> Value {
    if parts.is_empty() && tool_only {
        return Value::Null;
    }
    if parts.is_empty() {
        return Value::String(String::new());
    }
    if parts.len() == 1 {
        if let Some(text) = parts[0]
            .as_object()
            .and_then(|object| object.get("text"))
            .and_then(Value::as_str)
        {
            return Value::String(text.to_string());
        }
    }
    if parts.iter().all(|part| {
        part.as_object()
            .and_then(|object| object.get("text"))
            .and_then(Value::as_str)
            .is_some()
            && part
                .as_object()
                .and_then(|object| object.get("type"))
                .and_then(Value::as_str)
                .is_none_or(|part_type| part_type == "text")
    }) {
        return Value::String(
            parts
                .iter()
                .filter_map(|part| {
                    part.as_object()
                        .and_then(|object| object.get("text"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join(""),
        );
    }
    Value::Array(parts)
}

fn coalesce_openai_responses_text_content(content: Vec<Value>) -> Vec<Value> {
    if content.len() <= 1 {
        return content;
    }
    let mut text = String::new();
    let mut annotations = Vec::new();
    for part in &content {
        let Some(part_object) = part.as_object() else {
            return content;
        };
        let part_type = part_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(part_type, "output_text" | "text") {
            return content;
        }
        let Some(part_text) = part_object.get("text").and_then(Value::as_str) else {
            return content;
        };
        text.push_str(part_text);
        if let Some(part_annotations) = part_object.get("annotations").and_then(Value::as_array) {
            annotations.extend(part_annotations.iter().cloned());
        }
    }
    vec![json!({
        "type": "output_text",
        "text": text,
        "annotations": annotations,
    })]
}

pub(crate) fn openai_content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.as_object()
                    .and_then(|object| object.get("text"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub(crate) fn claude_generation_config(request: &Map<String, Value>) -> CanonicalGenerationConfig {
    CanonicalGenerationConfig {
        max_tokens: request.get("max_tokens").and_then(Value::as_u64),
        temperature: request.get("temperature").and_then(Value::as_f64),
        top_p: request.get("top_p").and_then(Value::as_f64),
        top_k: request.get("top_k").and_then(Value::as_u64),
        stop_sequences: request
            .get("stop")
            .or_else(|| request.get("stop_sequences"))
            .and_then(openai_stop_to_vec),
        ..CanonicalGenerationConfig::default()
    }
}

pub(crate) fn claude_tools_to_canonical(
    value: Option<&Value>,
) -> Option<(Vec<CanonicalToolDefinition>, Vec<Value>, Option<Value>)> {
    let Some(value) = value else {
        return Some((Vec::new(), Vec::new(), None));
    };
    let tools = value.as_array()?;
    let mut canonical = Vec::new();
    let mut builtin_tools = Vec::new();
    let mut web_search_options = None;
    for tool in tools {
        let tool_object = tool.as_object()?;
        let tool_type = tool_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if tool_type.starts_with("web_search") {
            web_search_options = claude_web_search_tool_to_openai_options(tool_object);
            builtin_tools.push(tool.clone());
            continue;
        }
        let name = tool_object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        canonical.push(CanonicalToolDefinition {
            name: name.to_string(),
            description: tool_object
                .get("description")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            parameters: tool_object.get("input_schema").cloned(),
            strict: None,
            extensions: {
                let mut extensions = claude_extensions(
                    tool_object,
                    &["type", "name", "description", "input_schema"],
                );
                if let Some(input_schema) = tool_object.get("input_schema").cloned() {
                    canonical_extension_object_mut(&mut extensions, "claude")
                        .insert("raw_input_schema".to_string(), input_schema);
                }
                extensions
            },
        });
    }
    Some((canonical, builtin_tools, web_search_options))
}

pub(crate) fn claude_web_search_tool_to_openai_options(tool: &Map<String, Value>) -> Option<Value> {
    let mut options = Map::new();
    if let Some(max_uses) = tool.get("max_uses").and_then(Value::as_u64) {
        let search_context_size = if max_uses <= 1 {
            "low"
        } else if max_uses <= 5 {
            "medium"
        } else {
            "high"
        };
        options.insert(
            "search_context_size".to_string(),
            Value::String(search_context_size.to_string()),
        );
    }
    if let Some(user_location) = tool.get("user_location").and_then(Value::as_object) {
        let mut approximate = Map::new();
        for field in ["city", "country", "region", "timezone"] {
            if let Some(value) = user_location.get(field).cloned() {
                approximate.insert(field.to_string(), value);
            }
        }
        if !approximate.is_empty() {
            options.insert(
                "user_location".to_string(),
                json!({
                    "type": "approximate",
                    "approximate": approximate,
                }),
            );
        }
    }
    Some(Value::Object(options))
}

pub(crate) fn claude_tool_choice_to_canonical(
    value: Option<&Value>,
) -> Option<CanonicalToolChoice> {
    match value {
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(CanonicalToolChoice::Auto),
            "any" => Some(CanonicalToolChoice::Required),
            "none" => Some(CanonicalToolChoice::None),
            _ => None,
        },
        Some(Value::Object(object)) => {
            if let Some(name) = object.get("name").and_then(Value::as_str) {
                return Some(CanonicalToolChoice::Tool {
                    name: name.to_string(),
                });
            }
            match object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "auto" => Some(CanonicalToolChoice::Auto),
                "any" => Some(CanonicalToolChoice::Required),
                "none" => Some(CanonicalToolChoice::None),
                "tool" => object.get("name").and_then(Value::as_str).map(|name| {
                    CanonicalToolChoice::Tool {
                        name: name.to_string(),
                    }
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn claude_parallel_tool_calls(value: Option<&Value>) -> Option<bool> {
    let object = value?.as_object()?;
    let choice_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if choice_type == "none" {
        return None;
    }
    object
        .get("disable_parallel_tool_use")
        .and_then(Value::as_bool)
        .map(|value| !value)
}

pub(crate) fn claude_thinking_to_canonical(
    request: &Map<String, Value>,
) -> Option<CanonicalThinkingConfig> {
    let thinking = request.get("thinking").and_then(Value::as_object);
    let output_config = request.get("output_config").and_then(Value::as_object);
    if thinking.is_none() && output_config.is_none() {
        return None;
    }
    let mut extensions = BTreeMap::new();
    if let Some(thinking) = thinking {
        extensions.insert("claude".to_string(), Value::Object(thinking.clone()));
    }
    if let Some(output_config) = output_config {
        extensions
            .entry("claude".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(object) = extensions.get_mut("claude").and_then(Value::as_object_mut) {
            object.insert(
                "output_config".to_string(),
                Value::Object(output_config.clone()),
            );
        }
    }
    if let Some(reasoning_effort) = output_config
        .and_then(|value| value.get("effort"))
        .and_then(Value::as_str)
        .and_then(claude_output_effort_to_openai_reasoning_effort)
        .or_else(|| {
            thinking
                .and_then(|value| value.get("budget_tokens"))
                .and_then(Value::as_u64)
                .map(map_thinking_budget_to_openai_reasoning_effort)
        })
    {
        extensions.insert(
            "openai".to_string(),
            json!({ "reasoning_effort": reasoning_effort }),
        );
    }
    Some(CanonicalThinkingConfig {
        enabled: thinking
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .is_none_or(|value| value == "enabled"),
        budget_tokens: thinking
            .and_then(|value| value.get("budget_tokens"))
            .and_then(Value::as_u64),
        extensions,
    })
}

pub(crate) fn claude_output_effort_to_openai_reasoning_effort(value: &str) -> Option<&'static str> {
    ReasoningEffort::parse(value).map(ReasoningEffort::as_openai_chat_value)
}

pub(crate) fn canonical_openai_reasoning_effort(
    thinking: &CanonicalThinkingConfig,
) -> Option<&str> {
    thinking
        .extensions
        .get("openai")
        .and_then(|value| value.get("reasoning_effort"))
        .and_then(Value::as_str)
        .or_else(|| {
            openai_responses_extension(&thinking.extensions)
                .and_then(|value| value.get("effort"))
                .and_then(Value::as_str)
        })
}

pub(crate) fn gemini_generation_config(value: Option<&Value>) -> CanonicalGenerationConfig {
    let Some(generation_config) = value.and_then(Value::as_object) else {
        return CanonicalGenerationConfig::default();
    };
    CanonicalGenerationConfig {
        max_tokens: gemini_value_by_case(generation_config, "maxOutputTokens", "max_output_tokens")
            .and_then(Value::as_u64),
        temperature: generation_config.get("temperature").and_then(Value::as_f64),
        top_p: gemini_value_by_case(generation_config, "topP", "top_p").and_then(Value::as_f64),
        top_k: gemini_value_by_case(generation_config, "topK", "top_k").and_then(Value::as_u64),
        stop_sequences: gemini_value_by_case(generation_config, "stopSequences", "stop_sequences")
            .and_then(openai_stop_to_vec),
        n: gemini_value_by_case(generation_config, "candidateCount", "candidate_count")
            .and_then(Value::as_u64),
        seed: generation_config.get("seed").and_then(Value::as_i64),
        ..CanonicalGenerationConfig::default()
    }
}

pub(crate) fn gemini_thinking_to_canonical(
    value: Option<&Value>,
) -> Option<CanonicalThinkingConfig> {
    let generation_config = value.and_then(Value::as_object)?;
    let thinking_config =
        gemini_value_by_case(generation_config, "thinkingConfig", "thinking_config")
            .and_then(Value::as_object)?;
    let budget_tokens = thinking_config
        .get("thinkingBudget")
        .or_else(|| thinking_config.get("thinking_budget"))
        .and_then(Value::as_u64);
    let thinking_level = thinking_config
        .get("thinkingLevel")
        .or_else(|| thinking_config.get("thinking_level"))
        .and_then(Value::as_str)
        .and_then(ReasoningEffort::parse)
        .map(ReasoningEffort::as_openai_chat_value);
    let mut extensions = BTreeMap::new();
    extensions.insert(
        "gemini".to_string(),
        json!({ "thinking_config": Value::Object(thinking_config.clone()) }),
    );
    if let Some(reasoning_effort) = budget_tokens
        .map(map_thinking_budget_to_openai_reasoning_effort)
        .or(thinking_level)
    {
        extensions.insert(
            "openai".to_string(),
            json!({ "reasoning_effort": reasoning_effort }),
        );
    }
    Some(CanonicalThinkingConfig {
        enabled: thinking_config
            .get("includeThoughts")
            .or_else(|| thinking_config.get("include_thoughts"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        budget_tokens,
        extensions,
    })
}

pub(crate) fn gemini_response_format_to_canonical(
    value: Option<&Value>,
) -> Option<CanonicalResponseFormat> {
    let generation_config = value.and_then(Value::as_object)?;
    let response_mime_type =
        gemini_value_by_case(generation_config, "responseMimeType", "response_mime_type")
            .and_then(Value::as_str)?;
    if response_mime_type != "application/json" {
        return None;
    }
    let json_schema = gemini_value_by_case(generation_config, "responseSchema", "response_schema")
        .map(|schema| {
            json!({
                "name": "response_schema",
                "schema": schema,
            })
        });
    Some(CanonicalResponseFormat {
        format_type: if json_schema.is_some() {
            "json_schema".to_string()
        } else {
            "json_object".to_string()
        },
        json_schema,
        extensions: BTreeMap::new(),
    })
}

pub(crate) type GeminiCanonicalTools = (
    Vec<CanonicalToolDefinition>,
    Vec<Value>,
    Option<Value>,
    Option<Value>,
    Option<Value>,
);

#[derive(Debug, Clone)]
pub(crate) struct GeminiGoogleSearchGrounding {
    pub source_field: &'static str,
    pub source_dialect: &'static str,
    pub legacy: bool,
    pub payload: Value,
    pub raw_payload: Value,
    pub output_payload: Value,
}

pub(crate) fn gemini_google_search_grounding(
    tool_object: &Map<String, Value>,
) -> Option<GeminiGoogleSearchGrounding> {
    for (field, source_dialect, legacy) in [
        ("googleSearch", "gemini_current", false),
        ("google_search", "gemini_current", false),
        ("googleSearchRetrieval", "vertex_legacy", true),
        ("google_search_retrieval", "vertex_legacy", true),
    ] {
        if let Some(raw_payload) = tool_object.get(field) {
            let raw_payload = normalize_gemini_tool_payload(raw_payload);
            let payload = lower_camelize_json_object_keys(&raw_payload);
            let output_payload = if legacy { json!({}) } else { payload.clone() };
            return Some(GeminiGoogleSearchGrounding {
                source_field: field,
                source_dialect,
                legacy,
                payload,
                raw_payload,
                output_payload,
            });
        }
    }
    None
}

pub(crate) fn gemini_google_search_grounding_extension(
    grounding: &GeminiGoogleSearchGrounding,
) -> Value {
    json!({
        "enabled": true,
        "source_field": grounding.source_field,
        "source_dialect": grounding.source_dialect,
        "payload": grounding.payload,
        "raw_payload": grounding.raw_payload,
        "legacy": grounding.legacy,
    })
}

fn normalize_gemini_tool_payload(payload: &Value) -> Value {
    match payload {
        Value::Null => json!({}),
        value => value.clone(),
    }
}

fn lower_camelize_json_object_keys(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    (
                        snake_to_lower_camel(key),
                        lower_camelize_json_object_keys(value),
                    )
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(lower_camelize_json_object_keys).collect())
        }
        other => other.clone(),
    }
}

fn snake_to_lower_camel(key: &str) -> String {
    let mut output = String::with_capacity(key.len());
    let mut uppercase_next = false;
    for character in key.chars() {
        if character == '_' {
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            for uppercase in character.to_uppercase() {
                output.push(uppercase);
            }
            uppercase_next = false;
        } else {
            output.push(character);
        }
    }
    output
}

fn gemini_builtin_tool_portion(tool_object: &Map<String, Value>) -> Option<Value> {
    let builtin = tool_object
        .iter()
        .filter(|(key, _)| {
            key.as_str() != "functionDeclarations" && key.as_str() != "function_declarations"
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    (!builtin.is_empty()).then_some(Value::Object(builtin))
}

pub(crate) fn gemini_tools_to_canonical(value: Option<&Value>) -> Option<GeminiCanonicalTools> {
    let Some(value) = value else {
        return Some((Vec::new(), Vec::new(), None, None, None));
    };
    let tools = value.as_array()?;
    let mut canonical = Vec::new();
    let mut builtin_tools = Vec::new();
    let mut web_search_options = None;
    let mut google_search_grounding = None;
    for tool in tools {
        let tool_object = tool.as_object()?;
        if let Some(grounding) = gemini_google_search_grounding(tool_object) {
            web_search_options = Some(json!({}));
            if google_search_grounding.is_none() {
                google_search_grounding =
                    Some(gemini_google_search_grounding_extension(&grounding));
            }
        }
        if let Some(builtin_tool) = gemini_builtin_tool_portion(tool_object) {
            builtin_tools.push(builtin_tool);
        }
        let declarations = tool_object
            .get("functionDeclarations")
            .or_else(|| tool_object.get("function_declarations"))
            .and_then(Value::as_array);
        let Some(declarations) = declarations else {
            continue;
        };
        for declaration in declarations {
            let declaration_object = declaration.as_object()?;
            let name = declaration_object
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            canonical.push(CanonicalToolDefinition {
                name: name.to_string(),
                description: declaration_object
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parameters: declaration_object.get("parameters").cloned(),
                strict: None,
                extensions: {
                    let mut extensions = gemini_extensions(
                        declaration_object,
                        &["name", "description", "parameters"],
                    );
                    if let Some(parameters) = declaration_object.get("parameters").cloned() {
                        canonical_extension_object_mut(&mut extensions, "gemini")
                            .insert("raw_parameters".to_string(), parameters);
                    }
                    extensions
                },
            });
        }
    }
    Some((
        canonical,
        builtin_tools,
        web_search_options,
        Some(value.clone()),
        google_search_grounding,
    ))
}

pub(crate) fn gemini_tool_choice_to_canonical(
    value: Option<&Value>,
) -> Option<CanonicalToolChoice> {
    let tool_config = value?.as_object()?;
    let function_config = tool_config
        .get("functionCallingConfig")
        .or_else(|| tool_config.get("function_calling_config"))?
        .as_object()?;
    if let Some(name) = function_config
        .get("allowedFunctionNames")
        .or_else(|| function_config.get("allowed_function_names"))
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(CanonicalToolChoice::Tool {
            name: name.to_string(),
        });
    }
    match function_config
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "NONE" => Some(CanonicalToolChoice::None),
        "AUTO" => Some(CanonicalToolChoice::Auto),
        "ANY" | "REQUIRED" => Some(CanonicalToolChoice::Required),
        _ => None,
    }
}

pub(crate) fn openai_generation_config(request: &Map<String, Value>) -> CanonicalGenerationConfig {
    CanonicalGenerationConfig {
        max_tokens: request
            .get("max_completion_tokens")
            .or_else(|| request.get("max_tokens"))
            .and_then(Value::as_u64),
        temperature: request.get("temperature").and_then(Value::as_f64),
        top_p: request.get("top_p").and_then(Value::as_f64),
        top_k: request.get("top_k").and_then(Value::as_u64),
        stop_sequences: request.get("stop").and_then(openai_stop_to_vec),
        n: request.get("n").and_then(Value::as_u64),
        presence_penalty: request.get("presence_penalty").and_then(Value::as_f64),
        frequency_penalty: request.get("frequency_penalty").and_then(Value::as_f64),
        seed: request.get("seed").and_then(Value::as_i64),
        logprobs: request.get("logprobs").and_then(Value::as_bool),
        top_logprobs: request.get("top_logprobs").and_then(Value::as_u64),
    }
}

pub(crate) fn openai_responses_generation_config(
    request: &Map<String, Value>,
) -> CanonicalGenerationConfig {
    let mut config = openai_generation_config(request);
    config.max_tokens = request.get("max_output_tokens").and_then(Value::as_u64);
    config
}

pub(crate) fn write_openai_generation_config(
    output: &mut Map<String, Value>,
    config: &CanonicalGenerationConfig,
) {
    if let Some(value) = config.max_tokens {
        output.insert("max_completion_tokens".to_string(), Value::from(value));
    }
    insert_f64(output, "temperature", config.temperature);
    insert_f64(output, "top_p", config.top_p);
    if let Some(value) = config.top_k {
        output.insert("top_k".to_string(), Value::from(value));
    }
    if let Some(values) = &config.stop_sequences {
        output.insert(
            "stop".to_string(),
            if values.len() == 1 {
                Value::String(values[0].clone())
            } else {
                Value::Array(values.iter().cloned().map(Value::String).collect())
            },
        );
    }
    if let Some(value) = config.n {
        output.insert("n".to_string(), Value::from(value));
    }
    insert_f64(output, "presence_penalty", config.presence_penalty);
    insert_f64(output, "frequency_penalty", config.frequency_penalty);
    if let Some(value) = config.seed {
        output.insert("seed".to_string(), Value::from(value));
    }
    if let Some(value) = config.logprobs {
        output.insert("logprobs".to_string(), Value::Bool(value));
    }
    if let Some(value) = config.top_logprobs {
        output.insert("top_logprobs".to_string(), Value::from(value));
    }
}

pub(crate) fn openai_tools_to_canonical(
    value: Option<&Value>,
) -> Option<Vec<CanonicalToolDefinition>> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let tools = value.as_array()?;
    let mut canonical = Vec::new();
    for tool in tools {
        let tool_object = tool.as_object()?;
        let tool_type = tool_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .trim()
            .to_ascii_lowercase();
        if tool_type == "function" {
            let function = tool_object.get("function").and_then(Value::as_object)?;
            canonical.push(CanonicalToolDefinition {
                name: function.get("name").and_then(Value::as_str)?.to_string(),
                description: function
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parameters: function.get("parameters").cloned(),
                strict: function.get("strict").and_then(Value::as_bool),
                extensions: openai_extensions(tool_object, &["type", "function"]),
            });
        } else if tool_type == "custom" {
            let custom = tool_object.get("custom").and_then(Value::as_object)?;
            let name = custom
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            canonical.push(CanonicalToolDefinition {
                name: name.to_string(),
                description: custom
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parameters: custom.get("parameters").cloned(),
                strict: custom.get("strict").and_then(Value::as_bool),
                extensions: BTreeMap::from([("openai".to_string(), tool.clone())]),
            });
        } else {
            return None;
        }
    }
    Some(canonical)
}

pub(crate) fn openai_responses_tools_to_canonical(
    value: Option<&Value>,
) -> Option<Vec<CanonicalToolDefinition>> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let tools = value.as_array()?;
    let mut canonical = Vec::new();
    for tool in tools {
        let tool_object = tool.as_object()?;
        let tool_type = tool_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .trim()
            .to_ascii_lowercase();
        if tool_type == "function" {
            if let Some(function) = tool_object.get("function").and_then(Value::as_object) {
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())?;
                let mut extensions =
                    openai_responses_extensions(tool_object, &["type", "function"]);
                let function_extensions = openai_responses_extensions(
                    function,
                    &["name", "description", "parameters", "strict"],
                );
                merge_tool_extensions(&mut extensions, function_extensions);
                canonical.push(CanonicalToolDefinition {
                    name: name.to_string(),
                    description: function
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    parameters: function.get("parameters").cloned(),
                    strict: function.get("strict").and_then(Value::as_bool),
                    extensions,
                });
                continue;
            }
            let name = tool_object
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            canonical.push(CanonicalToolDefinition {
                name: name.to_string(),
                description: tool_object
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parameters: tool_object.get("parameters").cloned(),
                strict: tool_object.get("strict").and_then(Value::as_bool),
                extensions: openai_responses_extensions(
                    tool_object,
                    &["type", "name", "description", "parameters", "strict"],
                ),
            });
        } else if tool_type == "custom" {
            let custom = tool_object.get("custom").and_then(Value::as_object);
            let name = tool_object
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| {
                    custom
                        .and_then(|value| value.get("name"))
                        .and_then(Value::as_str)
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let description = tool_object
                .get("description")
                .and_then(Value::as_str)
                .or_else(|| {
                    custom
                        .and_then(|value| value.get("description"))
                        .and_then(Value::as_str)
                })
                .map(ToOwned::to_owned);
            let parameters = tool_object
                .get("parameters")
                .or_else(|| custom.and_then(|value| value.get("parameters")))
                .filter(|value| value.is_object())
                .cloned();
            canonical.push(CanonicalToolDefinition {
                name: name.to_string(),
                description,
                parameters,
                strict: tool_object.get("strict").and_then(Value::as_bool),
                extensions: BTreeMap::from([(
                    OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
                    tool.clone(),
                )]),
            });
        } else if tool_type.starts_with("web_search") {
            canonical.push(CanonicalToolDefinition {
                name: tool_type,
                description: None,
                parameters: None,
                strict: None,
                extensions: BTreeMap::from([(
                    OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
                    tool.clone(),
                )]),
            });
        } else {
            let name = tool_object
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(tool_type.as_str());
            canonical.push(CanonicalToolDefinition {
                name: name.to_string(),
                description: tool_object
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parameters: None,
                strict: None,
                extensions: BTreeMap::from([(
                    OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
                    tool.clone(),
                )]),
            });
        }
    }
    Some(canonical)
}

fn merge_tool_extensions(target: &mut BTreeMap<String, Value>, source: BTreeMap<String, Value>) {
    for (namespace, value) in source {
        match (target.get_mut(&namespace), value) {
            (Some(Value::Object(target)), Value::Object(source)) => {
                target.extend(source);
            }
            (_, value) => {
                target.insert(namespace, value);
            }
        }
    }
}

pub(crate) fn canonical_tool_to_openai(tool: &CanonicalToolDefinition) -> Value {
    if let Some(raw) = tool
        .extensions
        .get("openai")
        .filter(|value| openai_tool_raw_type_is(value, "custom"))
    {
        return raw.clone();
    }
    if let Some(raw) = tool
        .extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            tool.extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .filter(|value| openai_tool_raw_type_is(value, "custom"))
    {
        return openai_responses_custom_tool_to_chat_tool(tool, raw);
    }
    let mut function = Map::new();
    function.insert("name".to_string(), Value::String(tool.name.clone()));
    if let Some(description) = &tool.description {
        function.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    if let Some(parameters) = &tool.parameters {
        function.insert("parameters".to_string(), parameters.clone());
    }
    if let Some(strict) = tool.strict {
        function.insert("strict".to_string(), Value::Bool(strict));
    }
    json!({
        "type": "function",
        "function": Value::Object(function),
    })
}

pub(crate) fn canonical_tool_is_openai_custom(tool: &CanonicalToolDefinition) -> bool {
    tool.extensions
        .get("openai")
        .or_else(|| tool.extensions.get(OPENAI_RESPONSES_EXTENSION_NAMESPACE))
        .or_else(|| {
            tool.extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .is_some_and(|value| openai_tool_raw_type_is(value, "custom"))
}

fn openai_tool_raw_type_is(value: &Value, expected_type: &str) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|tool_type| tool_type.eq_ignore_ascii_case(expected_type))
}

fn openai_responses_custom_tool_to_chat_tool(tool: &CanonicalToolDefinition, raw: &Value) -> Value {
    let mut custom = raw
        .get("custom")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            raw.as_object()
                .map(|object| {
                    object
                        .iter()
                        .filter(|(key, _)| key.as_str() != "type")
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                })
                .unwrap_or_default()
        });
    custom
        .entry("name".to_string())
        .or_insert_with(|| Value::String(tool.name.clone()));
    if let Some(description) = &tool.description {
        custom
            .entry("description".to_string())
            .or_insert_with(|| Value::String(description.clone()));
    }
    json!({
        "type": "custom",
        "custom": Value::Object(custom),
    })
}

pub(crate) fn openai_tool_choice_to_canonical(
    value: Option<&Value>,
) -> Option<CanonicalToolChoice> {
    match value {
        Some(Value::String(value)) => match value.as_str() {
            "auto" => Some(CanonicalToolChoice::Auto),
            "none" => Some(CanonicalToolChoice::None),
            "required" => Some(CanonicalToolChoice::Required),
            _ => None,
        },
        Some(Value::Object(object)) => object
            .get("function")
            .and_then(Value::as_object)
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .or_else(|| {
                object
                    .get("custom")
                    .and_then(Value::as_object)
                    .and_then(|custom| custom.get("name"))
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|name| CanonicalToolChoice::Tool {
                name: name.to_string(),
            }),
        _ => None,
    }
}

pub(crate) fn openai_responses_tool_choice_to_canonical(
    value: Option<&Value>,
) -> Option<CanonicalToolChoice> {
    match value {
        Some(Value::String(value)) => match value.as_str() {
            "auto" => Some(CanonicalToolChoice::Auto),
            "none" => Some(CanonicalToolChoice::None),
            "required" => Some(CanonicalToolChoice::Required),
            _ => None,
        },
        Some(Value::Object(object)) => {
            let choice_type = object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if choice_type == "function" {
                object
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        object
                            .get("function")
                            .and_then(Value::as_object)
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                    })
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|name| CanonicalToolChoice::Tool {
                        name: name.to_string(),
                    })
            } else if choice_type == "custom" {
                object
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        object
                            .get("custom")
                            .and_then(Value::as_object)
                            .and_then(|custom| custom.get("name"))
                            .and_then(Value::as_str)
                    })
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|name| CanonicalToolChoice::Tool {
                        name: name.to_string(),
                    })
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(crate) fn canonical_tool_choice_to_openai(choice: &CanonicalToolChoice) -> Value {
    match choice {
        CanonicalToolChoice::Auto => Value::String("auto".to_string()),
        CanonicalToolChoice::None => Value::String("none".to_string()),
        CanonicalToolChoice::Required => Value::String("required".to_string()),
        CanonicalToolChoice::Tool { name } => json!({
            "type": "function",
            "function": { "name": name },
        }),
    }
}

pub(crate) fn openai_tool_choice_raw_to_chat(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    match object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "function" => {
            raw_named_tool_choice_to_chat(object, "function").unwrap_or_else(|| value.clone())
        }
        "custom" => {
            raw_named_tool_choice_to_chat(object, "custom").unwrap_or_else(|| value.clone())
        }
        "allowed_tools" => raw_allowed_tool_choice_to_chat(object),
        _ => value.clone(),
    }
}

pub(crate) fn openai_tool_choice_raw_to_responses(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    match object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "function" => {
            raw_named_tool_choice_to_responses(object, "function").unwrap_or_else(|| value.clone())
        }
        "custom" => {
            raw_named_tool_choice_to_responses(object, "custom").unwrap_or_else(|| value.clone())
        }
        "allowed_tools" => raw_allowed_tool_choice_to_responses(object),
        _ => value.clone(),
    }
}

fn raw_named_tool_choice_to_chat(object: &Map<String, Value>, tool_type: &str) -> Option<Value> {
    let name = raw_tool_choice_name(object, tool_type)?;
    let mut named_tool = Map::new();
    named_tool.insert("name".to_string(), Value::String(name));
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String(tool_type.to_string()));
    out.insert(tool_type.to_string(), Value::Object(named_tool));
    Some(Value::Object(out))
}

fn raw_named_tool_choice_to_responses(
    object: &Map<String, Value>,
    tool_type: &str,
) -> Option<Value> {
    let name = raw_tool_choice_name(object, tool_type)?;
    Some(json!({
        "type": tool_type,
        "name": name,
    }))
}

fn raw_tool_choice_name(object: &Map<String, Value>, tool_type: &str) -> Option<String> {
    object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get(tool_type)
                .and_then(Value::as_object)
                .and_then(|tool| tool.get("name"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn raw_allowed_tool_choice_to_chat(object: &Map<String, Value>) -> Value {
    let mut allowed = object
        .get("allowed_tools")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| raw_allowed_tool_choice_payload(object));
    if let Some(tools) = allowed.get("tools").and_then(Value::as_array) {
        allowed.insert(
            "tools".to_string(),
            Value::Array(tools.iter().map(raw_allowed_tool_to_chat).collect()),
        );
    }
    json!({
        "type": "allowed_tools",
        "allowed_tools": Value::Object(allowed),
    })
}

fn raw_allowed_tool_choice_to_responses(object: &Map<String, Value>) -> Value {
    let mut out = object
        .get("allowed_tools")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| raw_allowed_tool_choice_payload(object));
    out.insert(
        "type".to_string(),
        Value::String("allowed_tools".to_string()),
    );
    if let Some(tools) = out.get("tools").and_then(Value::as_array) {
        out.insert(
            "tools".to_string(),
            Value::Array(tools.iter().map(raw_allowed_tool_to_responses).collect()),
        );
    }
    Value::Object(out)
}

fn raw_allowed_tool_choice_payload(object: &Map<String, Value>) -> Map<String, Value> {
    object
        .iter()
        .filter(|(key, _)| key.as_str() != "type")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn raw_allowed_tool_to_chat(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    let tool_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if tool_type == "function" || tool_type == "custom" {
        raw_named_tool_choice_to_chat(object, &tool_type).unwrap_or_else(|| value.clone())
    } else {
        value.clone()
    }
}

fn raw_allowed_tool_to_responses(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    let tool_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if tool_type == "function" || tool_type == "custom" {
        raw_named_tool_choice_to_responses(object, &tool_type).unwrap_or_else(|| value.clone())
    } else {
        value.clone()
    }
}

pub(crate) fn canonical_instructions_to_claude_system(
    instructions: &[CanonicalInstruction],
) -> Option<Value> {
    let instructions = instructions
        .iter()
        .filter(|instruction| !instruction.text.trim().is_empty())
        .collect::<Vec<_>>();
    if instructions.is_empty() {
        return None;
    }
    let mut blocks = Vec::new();
    let mut has_structured_extensions = false;
    for instruction in &instructions {
        let mut block = Map::new();
        block.insert("type".to_string(), Value::String("text".to_string()));
        block.insert("text".to_string(), Value::String(instruction.text.clone()));
        let extra = namespace_extension_object(&instruction.extensions, "claude", &block);
        if !extra.is_empty() {
            has_structured_extensions = true;
            block.extend(extra);
        }
        blocks.push(Value::Object(block));
    }
    if has_structured_extensions {
        Some(Value::Array(blocks))
    } else {
        Some(Value::String(
            instructions
                .iter()
                .map(|instruction| instruction.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        ))
    }
}

pub(crate) fn canonical_messages_to_claude(canonical: &CanonicalRequest) -> Option<Vec<Value>> {
    let mut messages = Vec::new();
    for message in &canonical.messages {
        let role = match message.role {
            CanonicalRole::Assistant => "assistant",
            CanonicalRole::Tool => "user",
            CanonicalRole::System | CanonicalRole::Developer => continue,
            CanonicalRole::Unknown | CanonicalRole::User => "user",
        };
        let blocks = canonical_blocks_to_claude(&message.content, message.role.clone())?;
        if blocks.is_empty() {
            continue;
        }
        messages.push(json!({
            "role": role,
            "content": simplify_canonical_claude_content(blocks),
        }));
    }
    Some(messages)
}

pub(crate) fn canonical_blocks_to_claude(
    blocks: &[CanonicalContentBlock],
    role: CanonicalRole,
) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    for block in blocks {
        if let Some(value) = canonical_block_to_claude(block, &role)? {
            out.push(value);
        }
    }
    Some(out)
}

pub(crate) fn canonical_block_to_claude(
    block: &CanonicalContentBlock,
    role: &CanonicalRole,
) -> Option<Option<Value>> {
    match block {
        CanonicalContentBlock::Text { text, extensions } => {
            if text.trim().is_empty() {
                return Some(None);
            }
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("text".to_string()));
            out.insert("text".to_string(), Value::String(text.clone()));
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::Thinking {
            text,
            signature,
            encrypted_content,
            extensions,
        } => {
            if let Some(data) = encrypted_content
                .as_ref()
                .filter(|value| !value.is_empty())
                .filter(|_| is_claude_thinking_block(extensions))
            {
                let mut out = Map::new();
                out.insert(
                    "type".to_string(),
                    Value::String("redacted_thinking".to_string()),
                );
                out.insert("data".to_string(), Value::String(data.clone()));
                out.extend(namespace_extension_object(extensions, "claude", &out));
                return Some(Some(Value::Object(out)));
            }
            if !matches!(role, CanonicalRole::Assistant) {
                if text.trim().is_empty() {
                    return Some(None);
                }
                return Some(Some(json!({
                    "type": "text",
                    "text": text,
                })));
            }
            if text.trim().is_empty() {
                return Some(None);
            }
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("thinking".to_string()));
            out.insert("thinking".to_string(), Value::String(text.clone()));
            if let Some(signature) = signature.as_ref().filter(|value| !value.is_empty()) {
                out.insert("signature".to_string(), Value::String(signature.clone()));
            }
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            extensions,
            ..
        } => {
            if !matches!(
                role,
                CanonicalRole::User | CanonicalRole::Tool | CanonicalRole::Unknown
            ) {
                return Some(Some(json!({
                    "type": "text",
                    "text": assistant_image_placeholder(url.as_deref(), data.is_some()),
                })));
            }
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("image".to_string()));
            out.insert(
                "source".to_string(),
                claude_source_value(media_type.as_deref(), data.as_deref(), url.as_deref())?,
            );
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::File {
            data,
            file_id,
            file_url,
            media_type,
            extensions,
            ..
        } => {
            if let Some(file_id) = file_id.as_ref().filter(|value| !value.is_empty()) {
                return Some(Some(json!({
                    "type": "text",
                    "text": format!("[File: {file_id}]"),
                })));
            }
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("document".to_string()));
            out.insert(
                "source".to_string(),
                claude_source_value(media_type.as_deref(), data.as_deref(), file_url.as_deref())?,
            );
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::Audio {
            data,
            media_type,
            format,
            extensions,
        } => {
            let fallback_media_type = format.as_ref().map(|value| format!("audio/{value}"));
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("document".to_string()));
            out.insert(
                "source".to_string(),
                claude_source_value(
                    media_type.as_deref().or(fallback_media_type.as_deref()),
                    data.as_deref(),
                    None,
                )?,
            );
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::ToolUse {
            id,
            name,
            input,
            extensions,
        } => {
            let input = remove_empty_pages_from_tool_input_value(name, input);
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("tool_use".to_string()));
            out.insert(
                "id".to_string(),
                Value::String(claude_compatible_tool_use_id(id)),
            );
            out.insert("name".to_string(), Value::String(name.clone()));
            out.insert("input".to_string(), input);
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::ToolResult {
            tool_use_id,
            output,
            content_text,
            is_error,
            extensions,
            ..
        } => {
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("tool_result".to_string()));
            out.insert(
                "tool_use_id".to_string(),
                Value::String(claude_compatible_tool_use_id(tool_use_id)),
            );
            out.insert(
                "content".to_string(),
                canonical_tool_result_content_to_claude(
                    output.as_ref(),
                    content_text.as_deref(),
                    role,
                    extensions,
                ),
            );
            if *is_error {
                out.insert("is_error".to_string(), Value::Bool(true));
            }
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::Unknown {
            payload,
            extensions,
            ..
        } if is_claude_raw_block(extensions) => Some(Some(payload.clone())),
        CanonicalContentBlock::Unknown { .. } => Some(None),
    }
}

fn canonical_tool_result_content_to_claude(
    output: Option<&Value>,
    content_text: Option<&str>,
    role: &CanonicalRole,
    extensions: &BTreeMap<String, Value>,
) -> Value {
    if matches!(role, CanonicalRole::Assistant) {
        return output
            .cloned()
            .unwrap_or_else(|| Value::String(content_text.unwrap_or_default().to_string()));
    }

    if is_openai_chat_tool_result(extensions) {
        let text = content_text
            .map(ToOwned::to_owned)
            .or_else(|| output.map(openai_responses_tool_output_text))
            .unwrap_or_default();
        return Value::String(non_empty_tool_result_text(&text));
    }

    match output {
        Some(Value::String(text)) => Value::String(non_empty_tool_result_text(text)),
        Some(Value::Array(parts)) if claude_tool_result_content_blocks_are_wire_safe(parts) => {
            Value::Array(parts.clone())
        }
        Some(Value::Null) => Value::String(non_empty_tool_result_text("")),
        Some(value) => serde_json::to_string(value)
            .map(|text| Value::String(non_empty_tool_result_text(&text)))
            .unwrap_or_else(|_| {
                Value::String(non_empty_tool_result_text(content_text.unwrap_or_default()))
            }),
        None => Value::String(non_empty_tool_result_text(content_text.unwrap_or_default())),
    }
}

fn is_openai_chat_tool_result(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(AETHER_EXTENSION_NAMESPACE)
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str)
        == Some(OPENAI_CHAT_TOOL_RESULT_SOURCE_MARKER)
}

fn non_empty_tool_result_text(text: &str) -> String {
    if text.trim().is_empty() {
        "(empty)".to_string()
    } else {
        text.to_string()
    }
}

fn claude_compatible_tool_use_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return "toolu_".to_string();
    }
    if trimmed.starts_with("toolu_") || trimmed.starts_with("call_") {
        trimmed.to_string()
    } else {
        format!("toolu_{trimmed}")
    }
}

fn claude_tool_result_content_blocks_are_wire_safe(parts: &[Value]) -> bool {
    !parts.is_empty()
        && parts.iter().all(|part| {
            part.as_object()
                .and_then(|object| object.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|block_type| {
                    matches!(block_type, "text" | "image" | "document" | "file")
                })
        })
}

pub(crate) fn claude_source_value(
    media_type: Option<&str>,
    data: Option<&str>,
    url: Option<&str>,
) -> Option<Value> {
    if let Some(data) = data.filter(|value| !value.is_empty()) {
        return Some(json!({
            "type": "base64",
            "media_type": media_type.unwrap_or("application/octet-stream"),
            "data": data,
        }));
    }
    url.filter(|value| !value.is_empty()).map(|url| {
        json!({
            "type": "url",
            "url": url,
        })
    })
}

pub(crate) fn simplify_canonical_claude_content(blocks: Vec<Value>) -> Value {
    if blocks.is_empty() {
        return Value::String(String::new());
    }
    let mut text_values = Vec::new();
    for block in &blocks {
        let Some(block_object) = block.as_object() else {
            return Value::Array(blocks);
        };
        if block_object.len() == 2
            && block_object.get("type").and_then(Value::as_str) == Some("text")
        {
            if let Some(text) = block_object.get("text").and_then(Value::as_str) {
                text_values.push(text.to_string());
                continue;
            }
        }
        return Value::Array(blocks);
    }
    Value::String(text_values.join("\n"))
}

pub(crate) fn compact_canonical_claude_messages(messages: Vec<Value>) -> Vec<Value> {
    let mut compact: Vec<Value> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(last) = compact.last_mut() {
            let last_role = last
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if last_role == role {
                merge_canonical_claude_message_content(last, message);
                continue;
            }
        }
        compact.push(message);
    }
    if compact
        .first()
        .and_then(|value| value.get("role"))
        .and_then(Value::as_str)
        .is_some_and(|value| value == "assistant")
    {
        compact.insert(0, json!({ "role": "user", "content": "" }));
    }
    compact
}

pub(crate) fn merge_canonical_claude_message_content(target: &mut Value, message: Value) {
    let Some(target_object) = target.as_object_mut() else {
        return;
    };
    let incoming_content = message.get("content").cloned().unwrap_or(Value::Null);
    let merged_blocks = extract_canonical_claude_content_blocks(target_object.get("content"))
        .into_iter()
        .chain(extract_canonical_claude_content_blocks(Some(
            &incoming_content,
        )))
        .collect::<Vec<_>>();
    target_object.insert(
        "content".to_string(),
        simplify_canonical_claude_content(merged_blocks),
    );
}

pub(crate) fn extract_canonical_claude_content_blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) if !text.is_empty() => vec![json!({
            "type": "text",
            "text": text,
        })],
        Some(Value::Array(blocks)) => blocks.clone(),
        _ => Vec::new(),
    }
}

pub(crate) fn canonical_tools_to_claude(canonical: &CanonicalRequest) -> Vec<Value> {
    let mut tools = canonical
        .tools
        .iter()
        .map(|tool| {
            let mut out = Map::new();
            out.insert("name".to_string(), Value::String(tool.name.clone()));
            if let Some(description) = &tool.description {
                out.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            out.insert(
                "input_schema".to_string(),
                tool.extensions
                    .get("claude")
                    .and_then(Value::as_object)
                    .and_then(|value| value.get("raw_input_schema"))
                    .cloned()
                    .unwrap_or_else(|| {
                        claude_input_schema_from_tool_parameters(tool.parameters.as_ref())
                    }),
            );
            let mut extra = namespace_extension_object(&tool.extensions, "claude", &out);
            extra.remove("raw_input_schema");
            out.extend(extra);
            Value::Object(out)
        })
        .collect::<Vec<_>>();
    if let Some(builtin_tools) = canonical
        .extensions
        .get("claude")
        .and_then(Value::as_object)
        .and_then(|value| value.get("builtin_tools"))
        .and_then(Value::as_array)
    {
        tools.extend(builtin_tools.iter().cloned());
    }
    tools
}

fn claude_input_schema_from_tool_parameters(parameters: Option<&Value>) -> Value {
    match parameters {
        Some(Value::Object(schema)) => {
            let mut schema = schema.clone();
            schema
                .entry("type".to_string())
                .or_insert_with(|| Value::String("object".to_string()));
            schema
                .entry("properties".to_string())
                .or_insert_with(|| json!({}));
            Value::Object(schema)
        }
        Some(Value::Null) | None => json!({"type": "object", "properties": {}}),
        Some(_) => json!({"type": "object", "properties": {}}),
    }
}

pub(crate) fn canonical_tool_choice_to_claude(
    choice: Option<&CanonicalToolChoice>,
    parallel_tool_calls: Option<bool>,
) -> Option<Value> {
    let mut out = match choice {
        Some(CanonicalToolChoice::None) => Some(json!({ "type": "none" })),
        Some(CanonicalToolChoice::Required) => Some(json!({ "type": "any" })),
        Some(CanonicalToolChoice::Auto) => Some(json!({ "type": "auto" })),
        Some(CanonicalToolChoice::Tool { name }) => Some(json!({
            "type": "tool",
            "name": name,
        })),
        None => parallel_tool_calls.map(|_| json!({ "type": "auto" })),
    }?;
    if let Some(parallel_tool_calls) = parallel_tool_calls {
        if let Some(object) = out.as_object_mut() {
            if object.get("type").and_then(Value::as_str) != Some("none") {
                object.insert(
                    "disable_parallel_tool_use".to_string(),
                    Value::Bool(!parallel_tool_calls),
                );
            }
        }
    }
    Some(out)
}

pub(crate) fn apply_gemini_request_extensions(
    output: &mut Value,
    extensions: &BTreeMap<String, Value>,
) -> Option<()> {
    let Some(gemini) = extensions.get("gemini").and_then(Value::as_object) else {
        return Some(());
    };
    let output_object = output.as_object_mut()?;

    let thinking_config = gemini.get("thinking_config").cloned();
    let response_modalities = gemini.get("response_modalities").cloned();
    let generation_config_extra = gemini
        .get("generation_config_extra")
        .and_then(Value::as_object)
        .cloned();
    if thinking_config.is_some()
        || response_modalities.is_some()
        || generation_config_extra.is_some()
    {
        let generation_config = output_object
            .entry("generationConfig".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()?;
        if let Some(thinking_config) = thinking_config {
            generation_config.insert("thinkingConfig".to_string(), thinking_config);
        }
        if let Some(response_modalities) = response_modalities {
            generation_config.insert("responseModalities".to_string(), response_modalities);
        }
        if let Some(extra) = generation_config_extra {
            for (key, value) in extra {
                generation_config.entry(key).or_insert(value);
            }
        }
    }

    if let Some(safety_settings) = gemini.get("safety_settings").cloned() {
        output_object.insert("safetySettings".to_string(), safety_settings);
    }
    if let Some(cached_content) = gemini.get("cached_content").cloned() {
        output_object.insert("cachedContent".to_string(), cached_content);
    }
    if let Some(raw_tools) = gemini.get("raw_tools").cloned() {
        if should_reuse_raw_gemini_tools(gemini) {
            output_object.insert("tools".to_string(), raw_tools);
        } else {
            output_object
                .entry("tools".to_string())
                .or_insert(raw_tools);
        }
    }
    if let Some(raw_tool_config) = gemini.get("raw_tool_config").cloned() {
        output_object.insert("toolConfig".to_string(), raw_tool_config);
    }
    for (key, value) in gemini {
        if GEMINI_REQUEST_EXTENSION_INTERNAL_KEYS.contains(&key.as_str()) {
            continue;
        }
        output_object
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    Some(())
}

const GEMINI_REQUEST_EXTENSION_INTERNAL_KEYS: &[&str] = &[
    "builtin_tools",
    "cached_content",
    "generation_config_extra",
    "grounding",
    "raw_tool_config",
    "raw_tools",
    "response_modalities",
    "safety_settings",
    "thinking_config",
];

fn should_reuse_raw_gemini_tools(gemini: &Map<String, Value>) -> bool {
    let Some(google_search) = gemini
        .get("grounding")
        .and_then(Value::as_object)
        .and_then(|grounding| grounding.get("google_search"))
        .and_then(Value::as_object)
    else {
        return true;
    };
    google_search
        .get("legacy")
        .and_then(Value::as_bool)
        .is_none_or(|legacy| !legacy)
        && google_search
            .get("source_field")
            .and_then(Value::as_str)
            .is_some_and(|source_field| source_field == "googleSearch")
}

pub(crate) fn assistant_image_placeholder(url: Option<&str>, has_data: bool) -> String {
    match (url, has_data) {
        (Some(url), false) if !url.trim().is_empty() => format!("[Image: {url}]"),
        _ => "[Image]".to_string(),
    }
}

pub(crate) fn openai_response_format_to_canonical(
    value: Option<&Value>,
) -> Option<CanonicalResponseFormat> {
    let object = value?.as_object()?;
    let format_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("text")
        .to_string();
    let json_schema = openai_response_format_json_schema(object, &format_type);
    let excluded_fields =
        if json_schema.is_some() && format_type.eq_ignore_ascii_case("json_schema") {
            &[
                "type",
                "json_schema",
                "name",
                "description",
                "schema",
                "strict",
            ][..]
        } else {
            &["type", "json_schema"][..]
        };
    Some(CanonicalResponseFormat {
        format_type,
        json_schema,
        extensions: openai_extensions(object, excluded_fields),
    })
}

fn openai_response_format_json_schema(
    object: &Map<String, Value>,
    format_type: &str,
) -> Option<Value> {
    if let Some(json_schema) = object.get("json_schema").cloned() {
        return Some(json_schema);
    }
    if !format_type.eq_ignore_ascii_case("json_schema") {
        return None;
    }
    let mut schema = Map::new();
    for field in ["name", "description", "schema", "strict"] {
        if let Some(value) = object.get(field) {
            schema.insert(field.to_string(), value.clone());
        }
    }
    (!schema.is_empty()).then_some(Value::Object(schema))
}

pub(crate) fn canonical_response_format_to_openai(value: &CanonicalResponseFormat) -> Value {
    let mut output = Map::new();
    output.insert("type".to_string(), Value::String(value.format_type.clone()));
    if let Some(json_schema) = &value.json_schema {
        output.insert("json_schema".to_string(), json_schema.clone());
    }
    Value::Object(output)
}

pub(crate) fn canonical_response_format_to_openai_responses(
    value: &CanonicalResponseFormat,
) -> Value {
    let mut output = Map::new();
    output.insert("type".to_string(), Value::String(value.format_type.clone()));
    if let Some(json_schema) = &value.json_schema {
        if value.format_type.eq_ignore_ascii_case("json_schema") {
            if let Some(schema_object) = json_schema.as_object() {
                output.extend(schema_object.clone());
            } else {
                output.insert("schema".to_string(), json_schema.clone());
            }
        } else {
            output.insert("json_schema".to_string(), json_schema.clone());
        }
    }
    Value::Object(output)
}

pub(crate) fn openai_usage_to_canonical(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .or_else(|| usage.get("output_tokens_details"))
        .and_then(Value::as_object)
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"))
        .and_then(Value::as_object)
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write_tokens = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"))
        .and_then(Value::as_object)
        .and_then(|details| {
            details
                .get("cache_write_tokens")
                .or_else(|| details.get("cached_creation_tokens"))
                .or_else(|| details.get("cache_creation_tokens"))
        })
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(CanonicalUsage {
        input_tokens,
        input_tokens_include_cache: cache_read_tokens > 0 || cache_write_tokens > 0,
        output_tokens,
        total_tokens: usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens.saturating_add(output_tokens)),
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
        extensions: openai_extensions(
            usage,
            &["prompt_tokens", "completion_tokens", "total_tokens"],
        ),
        ..CanonicalUsage::default()
    })
}

pub(crate) fn openai_responses_usage_to_canonical(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let mut canonical = openai_usage_to_canonical(value)?;
    let provider_fields = usage
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "input_tokens" | "output_tokens" | "total_tokens"
            )
        })
        .map(|(key, value)| {
            let value = if key == "input_tokens_details" {
                let mut details = value.as_object().cloned().unwrap_or_default();
                details.remove("cached_creation_tokens");
                details.remove("cache_creation_tokens");
                Value::Object(details)
            } else {
                value.clone()
            };
            (key.clone(), value)
        })
        .collect::<Map<String, Value>>();
    canonical.extensions = if provider_fields.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            Value::Object(provider_fields),
        )])
    };
    Some(canonical)
}

pub(crate) fn claude_usage_to_canonical(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| {
            details
                .get("thinking_tokens")
                .or_else(|| details.get("reasoning_tokens"))
        })
        .or_else(|| usage.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(CanonicalUsage {
        input_tokens,
        input_tokens_include_cache: false,
        output_tokens,
        total_tokens: input_tokens.saturating_add(output_tokens),
        cache_read_tokens,
        cache_write_tokens,
        cache_creation_ephemeral_5m_tokens: usage
            .get("cache_creation")
            .and_then(Value::as_object)
            .and_then(|value| value.get("ephemeral_5m_input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_creation_ephemeral_1h_tokens: usage
            .get("cache_creation")
            .and_then(Value::as_object)
            .and_then(|value| value.get("ephemeral_1h_input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        extensions: claude_extensions(
            usage,
            &[
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
                "cache_creation",
                "output_tokens_details",
                "reasoning_tokens",
            ],
        ),
        reasoning_tokens,
    })
}

pub(crate) fn gemini_usage_to_canonical(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("promptTokenCount")
        .or_else(|| usage.get("prompt_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let visible_output_tokens = usage
        .get("candidatesTokenCount")
        .or_else(|| usage.get("candidates_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("thoughtsTokenCount")
        .or_else(|| usage.get("thoughts_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cachedContentTokenCount")
        .or_else(|| usage.get("cached_content_token_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = visible_output_tokens + reasoning_tokens;
    Some(CanonicalUsage {
        input_tokens,
        input_tokens_include_cache: cache_read_tokens > 0,
        output_tokens,
        total_tokens: usage
            .get("totalTokenCount")
            .or_else(|| usage.get("total_token_count"))
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens + output_tokens),
        cache_read_tokens,
        reasoning_tokens,
        extensions: gemini_extensions(
            usage,
            &[
                "promptTokenCount",
                "prompt_token_count",
                "cachedContentTokenCount",
                "cached_content_token_count",
                "candidatesTokenCount",
                "candidates_token_count",
                "thoughtsTokenCount",
                "thoughts_token_count",
                "totalTokenCount",
                "total_token_count",
            ],
        ),
        ..CanonicalUsage::default()
    })
}

pub(crate) fn canonical_usage_to_openai(value: &CanonicalUsage) -> Value {
    let input_tokens = canonical_usage_total_input_tokens(value);
    let total_tokens = canonical_usage_total_tokens_for_inclusive_input(value, input_tokens);
    let mut output = json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": value.output_tokens,
        "total_tokens": total_tokens,
    });
    if value.reasoning_tokens > 0 {
        output["completion_tokens_details"] = json!({
            "reasoning_tokens": value.reasoning_tokens,
        });
    }
    if value.cache_read_tokens > 0 {
        output["prompt_tokens_details"] = json!({
            "cached_tokens": value.cache_read_tokens,
        });
    }
    if value.cache_write_tokens > 0 {
        if output.get("prompt_tokens_details").is_none() {
            output["prompt_tokens_details"] = json!({});
        }
        output["prompt_tokens_details"]["cache_write_tokens"] =
            Value::from(value.cache_write_tokens);
    }
    output
}

pub(crate) fn canonical_usage_to_openai_responses_usage(value: &CanonicalUsage) -> Value {
    let input_tokens = canonical_usage_total_input_tokens(value);
    let total_tokens = canonical_usage_total_tokens_for_inclusive_input(value, input_tokens);
    let mut output = json!({
        "input_tokens": input_tokens,
        "output_tokens": value.output_tokens,
        "total_tokens": total_tokens,
    });
    if value.reasoning_tokens > 0 {
        output["output_tokens_details"] = json!({
            "reasoning_tokens": value.reasoning_tokens,
        });
    }
    if value.cache_read_tokens > 0 {
        output["input_tokens_details"] = json!({
            "cached_tokens": value.cache_read_tokens,
        });
    }
    if value.cache_write_tokens > 0 {
        if output.get("input_tokens_details").is_none() {
            output["input_tokens_details"] = json!({});
        }
        output["input_tokens_details"]["cache_write_tokens"] =
            Value::from(value.cache_write_tokens);
    }
    if let (Some(output), Some(provider_fields)) = (
        output.as_object_mut(),
        openai_responses_extension(&value.extensions).and_then(Value::as_object),
    ) {
        merge_json_object_missing(output, provider_fields);
    }
    output
}

fn merge_json_object_missing(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (key, source_value) in source {
        match (target.get_mut(key), source_value) {
            (Some(Value::Object(target_object)), Value::Object(source_object)) => {
                merge_json_object_missing(target_object, source_object);
            }
            (Some(_), _) => {}
            (None, _) => {
                target.insert(key.clone(), source_value.clone());
            }
        }
    }
}

pub(crate) fn canonical_usage_to_claude(value: &CanonicalUsage) -> Value {
    let mut output = json!({
        "input_tokens": canonical_usage_uncached_input_tokens(value),
        "output_tokens": value.output_tokens,
    });
    if value.cache_read_tokens > 0 {
        output["cache_read_input_tokens"] = Value::from(value.cache_read_tokens);
    }
    if value.cache_write_tokens > 0 {
        output["cache_creation_input_tokens"] = Value::from(value.cache_write_tokens);
    }
    if value.cache_creation_ephemeral_5m_tokens > 0 || value.cache_creation_ephemeral_1h_tokens > 0
    {
        output["cache_creation"] = json!({
            "ephemeral_5m_input_tokens": value.cache_creation_ephemeral_5m_tokens,
            "ephemeral_1h_input_tokens": value.cache_creation_ephemeral_1h_tokens,
        });
    }
    if value.reasoning_tokens > 0 {
        output["output_tokens_details"] = json!({
            "thinking_tokens": value.reasoning_tokens,
        });
    }
    output
}

fn canonical_usage_cache_creation_tokens(value: &CanonicalUsage) -> u64 {
    if value.cache_write_tokens > 0 {
        value.cache_write_tokens
    } else {
        value
            .cache_creation_ephemeral_5m_tokens
            .saturating_add(value.cache_creation_ephemeral_1h_tokens)
    }
}

fn canonical_usage_cache_input_tokens(value: &CanonicalUsage) -> u64 {
    value
        .cache_read_tokens
        .saturating_add(canonical_usage_cache_creation_tokens(value))
}

fn canonical_usage_uncached_input_tokens(value: &CanonicalUsage) -> u64 {
    if value.input_tokens_include_cache {
        value
            .input_tokens
            .saturating_sub(canonical_usage_cache_input_tokens(value))
    } else {
        value.input_tokens
    }
}

pub(crate) fn canonical_usage_total_input_tokens(value: &CanonicalUsage) -> u64 {
    if value.input_tokens_include_cache {
        value.input_tokens
    } else {
        value
            .input_tokens
            .saturating_add(canonical_usage_cache_input_tokens(value))
    }
}

pub(crate) fn canonical_usage_total_tokens_for_inclusive_input(
    value: &CanonicalUsage,
    input_tokens: u64,
) -> u64 {
    if value.total_tokens > 0
        && (value.input_tokens_include_cache || canonical_usage_cache_input_tokens(value) == 0)
    {
        value.total_tokens
    } else {
        input_tokens.saturating_add(value.output_tokens)
    }
}

pub(crate) fn openai_finish_reason_to_canonical(
    value: Option<&str>,
) -> Option<CanonicalStopReason> {
    Some(match value? {
        "stop" => CanonicalStopReason::EndTurn,
        "length" => CanonicalStopReason::MaxTokens,
        "tool_calls" | "function_call" => CanonicalStopReason::ToolUse,
        "content_filter" => CanonicalStopReason::ContentFiltered,
        _ => CanonicalStopReason::Unknown,
    })
}

pub(crate) fn claude_stop_reason_to_canonical(value: Option<&str>) -> Option<CanonicalStopReason> {
    Some(match value? {
        "end_turn" => CanonicalStopReason::EndTurn,
        "max_tokens" => CanonicalStopReason::MaxTokens,
        "stop_sequence" => CanonicalStopReason::StopSequence,
        "tool_use" => CanonicalStopReason::ToolUse,
        "pause_turn" => CanonicalStopReason::PauseTurn,
        "refusal" => CanonicalStopReason::Refusal,
        "content_filtered" => CanonicalStopReason::ContentFiltered,
        _ => CanonicalStopReason::Unknown,
    })
}

pub(crate) fn gemini_stop_reason_to_canonical(value: &str) -> Option<CanonicalStopReason> {
    Some(match value.trim().to_ascii_uppercase().as_str() {
        "STOP" => CanonicalStopReason::EndTurn,
        "MAX_TOKENS" => CanonicalStopReason::MaxTokens,
        "SAFETY"
        | "RECITATION"
        | "LANGUAGE"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION" => CanonicalStopReason::ContentFiltered,
        "OTHER" => CanonicalStopReason::Unknown,
        _ => CanonicalStopReason::Unknown,
    })
}

pub(crate) fn canonical_stop_reason_to_openai(value: Option<&CanonicalStopReason>) -> &'static str {
    match value {
        Some(CanonicalStopReason::MaxTokens) => "length",
        Some(CanonicalStopReason::ToolUse) => "tool_calls",
        Some(CanonicalStopReason::ContentFiltered | CanonicalStopReason::Refusal) => {
            "content_filter"
        }
        _ => "stop",
    }
}

pub(crate) fn canonical_stop_reason_to_claude(value: Option<&CanonicalStopReason>) -> &'static str {
    match value {
        Some(CanonicalStopReason::MaxTokens) => "max_tokens",
        Some(CanonicalStopReason::StopSequence) => "stop_sequence",
        Some(CanonicalStopReason::ToolUse) => "tool_use",
        Some(CanonicalStopReason::PauseTurn) => "pause_turn",
        Some(CanonicalStopReason::Refusal) => "refusal",
        Some(CanonicalStopReason::ContentFiltered) => "content_filtered",
        _ => "end_turn",
    }
}

pub(crate) fn openai_stop_to_vec(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::String(text) => Some(vec![text.clone()]),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect(),
        ),
        _ => None,
    }
}

pub(crate) fn parse_jsonish_value(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        Some(value) => value.clone(),
        None => json!({}),
    }
}

pub(crate) fn openai_responses_tool_output_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn canonicalize_tool_arguments(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

pub(crate) fn split_data_url(
    value: Option<String>,
    fallback_media_type: Option<String>,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(value) = value else {
        return (fallback_media_type, None, None);
    };
    if let Some(rest) = value.strip_prefix("data:") {
        if let Some((media_type, data)) = rest.split_once(";base64,") {
            return (Some(media_type.to_string()), Some(data.to_string()), None);
        }
    }
    (fallback_media_type, None, Some(value))
}

pub(crate) fn media_data_or_url(
    media_type: &Option<String>,
    data: &Option<String>,
    url: &Option<String>,
) -> String {
    if let Some(data) = data {
        return format!(
            "data:{};base64,{}",
            media_type.as_deref().unwrap_or("application/octet-stream"),
            data
        );
    }
    url.clone().unwrap_or_default()
}

pub(crate) fn offset_openai_annotation_indices(annotation: &Value, offset: i64) -> Value {
    let Some(object) = annotation.as_object() else {
        return annotation.clone();
    };
    let mut adjusted = object.clone();
    for key in [
        "start_index",
        "end_index",
        "start_char",
        "end_char",
        "index",
    ] {
        if let Some(value) = adjusted.get(key).and_then(Value::as_i64) {
            adjusted.insert(key.to_string(), Value::from(value + offset));
        }
    }
    Value::Object(adjusted)
}

pub(crate) fn insert_f64(output: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        if let Some(number) = serde_json::Number::from_f64(value) {
            output.insert(key.to_string(), Value::Number(number));
        }
    }
}

pub(crate) fn openai_extensions(
    object: &Map<String, Value>,
    handled_keys: &[&str],
) -> BTreeMap<String, Value> {
    let handled = handled_keys
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let raw = object
        .iter()
        .filter(|(key, _)| !handled.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if raw.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([("openai".to_string(), Value::Object(raw))])
    }
}

pub(crate) fn claude_extensions(
    object: &Map<String, Value>,
    handled_keys: &[&str],
) -> BTreeMap<String, Value> {
    let handled = handled_keys
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let raw = object
        .iter()
        .filter(|(key, _)| !handled.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if raw.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([("claude".to_string(), Value::Object(raw))])
    }
}

pub(crate) fn openai_responses_extensions(
    object: &Map<String, Value>,
    handled_keys: &[&str],
) -> BTreeMap<String, Value> {
    let mut extensions = openai_extensions(object, handled_keys);
    if let Some(raw) = extensions.remove("openai") {
        extensions.insert(OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(), raw);
    }
    extensions
}

pub(crate) fn gemini_extensions(
    object: &Map<String, Value>,
    handled_keys: &[&str],
) -> BTreeMap<String, Value> {
    let handled = handled_keys
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let raw = object
        .iter()
        .filter(|(key, _)| !handled.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if raw.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([("gemini".to_string(), Value::Object(raw))])
    }
}

const GEMINI_MAPPED_GENERATION_CONFIG_KEYS: &[&str] = &[
    "maxOutputTokens",
    "max_output_tokens",
    "temperature",
    "topP",
    "top_p",
    "topK",
    "top_k",
    "candidateCount",
    "candidate_count",
    "seed",
    "stopSequences",
    "stop_sequences",
    "thinkingConfig",
    "thinking_config",
    "responseMimeType",
    "response_mime_type",
    "responseSchema",
    "response_schema",
    "responseModalities",
    "response_modalities",
];

pub(crate) fn gemini_value_by_case<'a>(
    object: &'a Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a Value> {
    object.get(camel).or_else(|| object.get(snake))
}

pub(crate) fn gemini_generation_config_extra(
    generation_config: &Map<String, Value>,
) -> Map<String, Value> {
    generation_config
        .iter()
        .filter(|(key, _)| {
            !GEMINI_MAPPED_GENERATION_CONFIG_KEYS
                .iter()
                .any(|candidate| candidate == &key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub(crate) fn extract_gemini_model_from_path(path: &str) -> Option<String> {
    let marker = "/models/";
    let start = path.find(marker)? + marker.len();
    let tail = &path[start..];
    let end = tail.find(':').unwrap_or(tail.len());
    let model = tail[..end].trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

pub(crate) fn canonical_extension_object_mut<'a>(
    extensions: &'a mut BTreeMap<String, Value>,
    namespace: &str,
) -> &'a mut Map<String, Value> {
    let entry = extensions
        .entry(namespace.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("extension namespace object")
}

pub(crate) fn namespace_extension_object(
    extensions: &BTreeMap<String, Value>,
    namespace: &str,
    existing: &Map<String, Value>,
) -> Map<String, Value> {
    extensions
        .get(namespace)
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter(|(key, _)| !existing.contains_key(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn openai_responses_extension(extensions: &BTreeMap<String, Value>) -> Option<&Value> {
    extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| extensions.get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE))
}

pub(crate) fn openai_service_tier_extension(
    extensions: &BTreeMap<String, Value>,
) -> Option<&Value> {
    [
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
        "openai",
    ]
    .into_iter()
    .find_map(|namespace| {
        extensions
            .get(namespace)
            .and_then(Value::as_object)
            .and_then(|object| object.get("service_tier"))
    })
}

pub(crate) fn openai_responses_item_extension_object(
    extensions: &BTreeMap<String, Value>,
    existing: &Map<String, Value>,
) -> Map<String, Value> {
    openai_responses_extension(extensions)
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter(|(key, _)| {
                    !existing.contains_key(*key) && !matches!(key.as_str(), "item_id" | "item_type")
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn strip_claude_billing_header(text: &str) -> String {
    let trimmed = text.trim();
    let prefix = "x-anthropic-billing-header:";
    if !trimmed.to_ascii_lowercase().starts_with(prefix) {
        return trimmed.to_string();
    }
    let remainder = trimmed
        .split_once('\n')
        .map(|(_, rest)| rest.trim_start())
        .unwrap_or_default();
    remainder.trim_start_matches('\n').trim().to_string()
}

fn rerank_document_is_empty(value: &Value) -> bool {
    match value {
        Value::String(text) => text.trim().is_empty(),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.trim().is_empty()),
        Value::Null => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_request_unknown_block_count, canonical_response_unknown_block_count,
        canonical_to_claude_request, canonical_to_claude_response, canonical_to_gemini_request,
        canonical_to_gemini_response, canonical_to_openai_chat_request,
        canonical_to_openai_chat_response, canonical_to_openai_responses_request,
        canonical_to_openai_responses_response, canonical_unknown_block_count,
        from_claude_to_canonical_request, from_claude_to_canonical_response,
        from_gemini_to_canonical_request, from_gemini_to_canonical_response,
        from_openai_chat_to_canonical_request, from_openai_chat_to_canonical_response,
        from_openai_responses_to_canonical_request, from_openai_responses_to_canonical_response,
        CanonicalContentBlock, CanonicalEmbedding, CanonicalEmbeddingContent,
        CanonicalEmbeddingInput, CanonicalEmbeddingRequest, CanonicalRole, CanonicalUsage,
    };
    use serde_json::{json, Value};

    #[test]
    fn canonical_embedding_request_accepts_axonhub_input_shapes() {
        for input in [
            json!("hello"),
            json!(["hello", "world"]),
            json!([1, 2, 3]),
            json!([[1, 2], [3, 4]]),
        ] {
            let request = json!({
                "model": "text-embedding-3-small",
                "embedding": {
                    "input": input,
                    "encoding_format": "float",
                    "dimensions": 3
                }
            });
            let canonical = serde_json::from_value::<super::CanonicalRequest>(request)
                .expect("embedding request should deserialize");
            assert!(canonical.embedding.is_some());
            assert!(canonical.messages.is_empty());
            let encoded = serde_json::to_value(&canonical).expect("serialize");
            assert!(encoded.get("messages").is_some());
            assert!(encoded.get("embedding").is_some());
        }
    }

    #[test]
    fn embedding_wire_request_accepts_all_axonhub_input_shapes() {
        let cases = [
            (
                json!("hello"),
                "single string",
                CanonicalEmbeddingInput::String("hello".to_string()),
            ),
            (
                json!(["hello", "world"]),
                "string array",
                CanonicalEmbeddingInput::StringArray(vec![
                    "hello".to_string(),
                    "world".to_string(),
                ]),
            ),
            (
                json!([1, 2, 3]),
                "token array",
                CanonicalEmbeddingInput::TokenArray(vec![1, 2, 3]),
            ),
            (
                json!([[1, 2], [3, 4]]),
                "nested token array",
                CanonicalEmbeddingInput::TokenArrayArray(vec![vec![1, 2], vec![3, 4]]),
            ),
            (
                json!([
                    {"text": "white running shoes"},
                    {"image": "https://example.com/shoe.png"},
                    {"video": "https://example.com/demo.mp4"},
                    {"multi_images": ["https://example.com/a.png", "https://example.com/b.png"]}
                ]),
                "multimodal array",
                CanonicalEmbeddingInput::Multimodal(vec![
                    CanonicalEmbeddingContent {
                        text: Some("white running shoes".to_string()),
                        image: None,
                        video: None,
                        multi_images: None,
                    },
                    CanonicalEmbeddingContent {
                        text: None,
                        image: Some("https://example.com/shoe.png".to_string()),
                        video: None,
                        multi_images: None,
                    },
                    CanonicalEmbeddingContent {
                        text: None,
                        image: None,
                        video: Some("https://example.com/demo.mp4".to_string()),
                        multi_images: None,
                    },
                    CanonicalEmbeddingContent {
                        text: None,
                        image: None,
                        video: None,
                        multi_images: Some(vec![
                            "https://example.com/a.png".to_string(),
                            "https://example.com/b.png".to_string(),
                        ]),
                    },
                ]),
            ),
        ];

        for (input, label, expected_input) in cases {
            let body = json!({
                "model": "text-embedding-3-small",
                "input": input
            });
            let canonical = super::from_embedding_to_canonical_request(&body, "openai")
                .unwrap_or_else(|| panic!("{label} should parse"));

            assert_eq!(
                canonical.embedding.expect("embedding request").input,
                expected_input,
                "{label} should preserve its canonical variant"
            );
            assert!(canonical.messages.is_empty());
        }
    }

    #[test]
    fn embedding_wire_request_rejects_empty_invalid_or_chat_payloads() {
        for body in [
            json!({"model": "text-embedding-3-small", "input": "   "}),
            json!({"model": "text-embedding-3-small", "input": []}),
            json!({"model": "text-embedding-3-small", "input": [1, "two"]}),
            json!({"model": "text-embedding-3-small", "input": [[1], []]}),
            json!({"model": "text-embedding-3-small", "input": [{"image": "   "}]}),
            json!({"model": "text-embedding-3-small", "input": [{"multi_images": []}]}),
            json!({"model": "text-embedding-3-small", "input": ["hello", {"image": "https://example.com/a.png"}]}),
            json!({"model": "", "input": "hello"}),
            json!({"input": "hello"}),
            json!({"model": "text-embedding-3-small", "messages": []}),
        ] {
            assert!(
                super::from_embedding_to_canonical_request(&body, "openai").is_none(),
                "invalid embedding payload should be rejected: {body}"
            );
        }
    }

    #[test]
    fn embedding_openai_request_response_roundtrip_stays_non_chat() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": ["hello", "world"],
            "encoding_format": "float",
            "dimensions": 2,
            "user": "user-1",
            "extra": true
        });
        let canonical =
            super::from_embedding_to_canonical_request(&body, "openai").expect("embedding request");
        assert_eq!(canonical.model, "text-embedding-3-small");
        assert!(canonical.messages.is_empty());
        assert!(matches!(
            canonical.embedding.as_ref().map(|embedding| &embedding.input),
            Some(CanonicalEmbeddingInput::StringArray(values)) if values == &vec!["hello".to_string(), "world".to_string()]
        ));

        let rebuilt =
            super::canonical_to_embedding_request(&canonical, "upstream-embedding", "openai")
                .expect("openai embedding request");
        assert_eq!(rebuilt["model"], "upstream-embedding");
        assert_eq!(rebuilt["input"], json!(["hello", "world"]));
        assert!(rebuilt.get("messages").is_none());

        let response = json!({
            "object": "list",
            "model": "upstream-embedding",
            "data": [
                {"object": "embedding", "index": 0, "embedding": [0.1, 0.2]},
                {"object": "embedding", "index": 1, "embedding": [0.3, 0.4]}
            ],
            "usage": {"prompt_tokens": 4, "total_tokens": 4}
        });
        let canonical_response = super::from_embedding_to_canonical_response(&response, "openai")
            .expect("embedding response");
        assert_eq!(canonical_response.embeddings.len(), 2);
        let emitted = super::canonical_to_embedding_response(&canonical_response, "openai")
            .expect("embedding response output");
        assert_eq!(emitted["data"][0]["embedding"], json!([0.1, 0.2]));
        assert!(emitted.get("choices").is_none());
        assert!(emitted.get("messages").is_none());
    }

    #[test]
    fn embedding_provider_request_emitters_preserve_provider_contracts() {
        let canonical = super::CanonicalRequest {
            model: "text-embedding-3-small".to_string(),
            embedding: Some(CanonicalEmbeddingRequest {
                input: CanonicalEmbeddingInput::StringArray(vec![
                    "alpha".to_string(),
                    "beta".to_string(),
                ]),
                encoding_format: Some("float".to_string()),
                dimensions: Some(2),
                task: None,
                user: None,
                parameters: None,
                extensions: Default::default(),
            }),
            ..Default::default()
        };

        let jina = super::canonical_to_embedding_request(&canonical, "jina-embeddings-v3", "jina")
            .expect("jina embedding request");
        assert_eq!(jina["task"], "text-matching");
        assert_eq!(jina["input"], json!(["alpha", "beta"]));

        let gemini =
            super::canonical_to_embedding_request(&canonical, "gemini-embedding-001", "gemini")
                .expect("gemini embedding request");
        assert!(gemini.get("model").is_none());
        assert_eq!(
            gemini["requests"][0]["model"],
            "models/gemini-embedding-001"
        );
        assert_eq!(
            gemini["requests"][0]["content"]["parts"][0]["text"],
            "alpha"
        );
        assert_eq!(gemini["requests"][0]["outputDimensionality"], 2);
        assert!(gemini.get("messages").is_none());

        let doubao = super::canonical_to_embedding_request(
            &canonical,
            "doubao-embedding-text-240515",
            "doubao",
        )
        .expect("doubao embedding request");
        assert_eq!(doubao["input"], json!(["alpha", "beta"]));
        assert!(doubao.get("messages").is_none());
    }

    #[test]
    fn embedding_provider_request_emitters_cover_golden_payload_variants() {
        let single = super::CanonicalRequest {
            model: "text-embedding-3-small".to_string(),
            embedding: Some(CanonicalEmbeddingRequest {
                input: CanonicalEmbeddingInput::String("alpha".to_string()),
                encoding_format: Some("float".to_string()),
                dimensions: Some(1536),
                task: Some("retrieval.passage".to_string()),
                user: Some("user-1".to_string()),
                parameters: None,
                extensions: Default::default(),
            }),
            ..Default::default()
        };

        let openai =
            super::canonical_to_embedding_request(&single, "text-embedding-3-large", "openai")
                .expect("openai embedding request");
        assert_eq!(openai["model"], "text-embedding-3-large");
        assert_eq!(openai["input"], "alpha");
        assert_eq!(openai["encoding_format"], "float");
        assert_eq!(openai["dimensions"], 1536);
        assert_eq!(openai["user"], "user-1");
        assert_eq!(openai["task"], "retrieval.passage");

        let jina = super::canonical_to_embedding_request(&single, "jina-embeddings-v3", "jina")
            .expect("jina embedding request");
        assert_eq!(jina["task"], "retrieval.passage");
        assert_eq!(jina["input"], "alpha");

        let gemini =
            super::canonical_to_embedding_request(&single, "gemini-embedding-001", "gemini")
                .expect("gemini single embedding request");
        assert_eq!(gemini["model"], "gemini-embedding-001");
        assert_eq!(gemini["content"]["parts"][0]["text"], "alpha");
        assert!(gemini.get("requests").is_none());

        let doubao = super::canonical_to_embedding_request(
            &single,
            "doubao-embedding-text-240515",
            "doubao",
        )
        .expect("doubao embedding request");
        assert_eq!(doubao["model"], "doubao-embedding-text-240515");
        assert_eq!(doubao["input"], json!(["alpha"]));
        assert_eq!(doubao["dimensions"], 1536);
    }

    #[test]
    fn gemini_and_doubao_embedding_emitters_reject_token_inputs() {
        for input in [
            CanonicalEmbeddingInput::TokenArray(vec![1, 2, 3]),
            CanonicalEmbeddingInput::TokenArrayArray(vec![vec![1, 2], vec![3, 4]]),
        ] {
            let canonical = super::CanonicalRequest {
                model: "token-model".to_string(),
                embedding: Some(CanonicalEmbeddingRequest {
                    input,
                    encoding_format: None,
                    dimensions: None,
                    task: None,
                    user: None,
                    parameters: None,
                    extensions: Default::default(),
                }),
                ..Default::default()
            };

            assert!(super::canonical_to_embedding_request(
                &canonical,
                "gemini-embedding-001",
                "gemini"
            )
            .is_none());
            assert!(super::canonical_to_embedding_request(
                &canonical,
                "doubao-embedding",
                "doubao"
            )
            .is_none());
        }
    }

    #[test]
    fn embedding_response_parser_rejects_error_and_malformed_vectors() {
        for body in [
            json!({"error": {"message": "bad"}}),
            json!({"object": "list"}),
            json!({"data": [{"object": "embedding", "embedding": [0.1, "bad"]}]}),
            json!({"data": [{"object": "embedding"}]}),
        ] {
            assert!(
                super::from_embedding_to_canonical_response(&body, "openai").is_none(),
                "malformed embedding response should be rejected: {body}"
            );
        }
    }

    #[test]
    fn embedding_response_parser_uses_openai_fallback_fields() {
        let body = json!({
            "object": "list",
            "data": [
                {"object": "embedding", "embedding": [0.1, 0.2]},
                {"object": "embedding", "index": 7, "embedding": [0.3, 0.4]}
            ]
        });

        let canonical = super::from_embedding_to_canonical_response(&body, "openai")
            .expect("fallback embedding response");
        assert_eq!(canonical.id, "embd-unknown");
        assert_eq!(canonical.model, "unknown");
        assert_eq!(canonical.embeddings[0].index, 0);
        assert_eq!(canonical.embeddings[1].index, 7);
        assert!(super::canonical_to_embedding_response(&canonical, "jina").is_some());
        assert!(super::canonical_to_embedding_response(&canonical, "gemini").is_none());
        assert!(super::canonical_to_embedding_response(&canonical, "doubao").is_none());
    }

    #[test]
    fn embedding_response_contract_serializes_vectors_without_chat_outputs() {
        let response = super::CanonicalEmbeddingResponse {
            id: "embd-1".to_string(),
            model: "text-embedding-3-small".to_string(),
            embeddings: vec![CanonicalEmbedding {
                index: 0,
                embedding: vec![0.1, 0.2, 0.3],
                extensions: Default::default(),
            }],
            usage: Some(CanonicalUsage {
                input_tokens: 3,
                total_tokens: 3,
                ..Default::default()
            }),
            extensions: Default::default(),
        };

        let encoded = serde_json::to_value(&response).expect("serialize");
        assert_eq!(
            encoded["embeddings"][0]["embedding"],
            json!([0.1, 0.2, 0.3])
        );
        assert!(encoded.get("choices").is_none());
        let decoded = serde_json::from_value::<super::CanonicalEmbeddingResponse>(encoded)
            .expect("deserialize");
        assert_eq!(decoded, response);
    }

    #[test]
    fn canonical_request_preserves_openai_multimodal_tools_and_extensions() {
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "system", "content": "Be exact.", "cache_control": {"type": "ephemeral"}},
                {"role": "developer", "content": [{"type": "text", "text": "Prefer short answers."}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "inspect this"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo=", "detail": "high"}},
                    {"type": "file", "file": {"file_data": "data:application/pdf;base64,JVBERi0x", "filename": "a.pdf"}},
                    {"type": "input_audio", "input_audio": {"data": "AAAA", "format": "mp3"}},
                    {"type": "future_part", "value": 1}
                ]},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{\"q\":\"rust\"}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": "{\"ok\":true}"}
            ],
            "tools": [{
                "type": "function",
                "function": {"name": "lookup", "description": "Lookup", "parameters": {"type": "object"}}
            }],
            "tool_choice": {"type": "function", "function": {"name": "lookup"}},
            "max_completion_tokens": 42,
            "temperature": 0.2,
            "reasoning_effort": "high",
            "metadata": {"trace": "abc"},
            "vendor_extra": true
        });

        let canonical = from_openai_chat_to_canonical_request(&request).expect("canonical request");
        assert_eq!(canonical.model, "gpt-5");
        assert_eq!(canonical.instructions.len(), 2);
        assert_eq!(canonical.instructions[1].role, CanonicalRole::Developer);
        assert_eq!(canonical.generation.max_tokens, Some(42));
        assert_eq!(canonical.tools[0].name, "lookup");
        assert!(canonical.thinking.is_some());
        assert!(canonical.extensions.contains_key("openai"));

        let user_blocks = &canonical.messages[0].content;
        assert!(
            matches!(user_blocks[1], CanonicalContentBlock::Image { ref data, ref media_type, .. } if data.as_deref() == Some("iVBORw0KGgo=") && media_type.as_deref() == Some("image/png"))
        );
        assert!(
            matches!(user_blocks[2], CanonicalContentBlock::File { ref data, ref filename, .. } if data.as_deref() == Some("JVBERi0x") && filename.as_deref() == Some("a.pdf"))
        );
        assert!(
            matches!(user_blocks[3], CanonicalContentBlock::Audio { ref data, ref format, .. } if data.as_deref() == Some("AAAA") && format.as_deref() == Some("mp3"))
        );
        assert_eq!(canonical_unknown_block_count(user_blocks), 1);
        assert_eq!(canonical_request_unknown_block_count(&canonical), 1);

        let rebuilt = canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(rebuilt["model"], "gpt-5");
        assert_eq!(rebuilt["messages"][0]["role"], "system");
        assert_eq!(rebuilt["messages"][1]["role"], "system");
        assert_eq!(
            rebuilt["messages"][2]["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
        assert_eq!(
            rebuilt["messages"][3]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"rust\"}"
        );
        assert_eq!(rebuilt["vendor_extra"], true);
    }

    #[test]
    fn openai_chat_request_adapter_preserves_custom_tools_and_choices() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "run a custom tool"}],
            "tools": [{
                "type": "custom",
                "custom": {
                    "name": "shell_command",
                    "description": "Run a shell command",
                    "format": {
                        "type": "text"
                    }
                }
            }],
            "tool_choice": {
                "type": "custom",
                "custom": {"name": "shell_command"}
            }
        });

        let canonical = from_openai_chat_to_canonical_request(&request).expect("canonical request");
        assert_eq!(canonical.tools.len(), 1);
        assert_eq!(canonical.tools[0].name, "shell_command");
        assert!(matches!(
            canonical.tool_choice,
            Some(super::CanonicalToolChoice::Tool { ref name }) if name == "shell_command"
        ));

        let rebuilt = canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(rebuilt["tools"][0]["type"], "custom");
        assert_eq!(rebuilt["tools"][0]["custom"]["name"], "shell_command");
        assert_eq!(rebuilt["tools"][0]["custom"]["format"]["type"], "text");
        assert_eq!(rebuilt["tool_choice"]["type"], "custom");
        assert_eq!(rebuilt["tool_choice"]["custom"]["name"], "shell_command");

        let responses = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(responses["tools"][0]["type"], "custom");
        assert_eq!(responses["tools"][0]["name"], "shell_command");
        assert_eq!(responses["tools"][0]["format"]["type"], "text");
        assert_eq!(responses["tool_choice"]["type"], "custom");
        assert_eq!(responses["tool_choice"]["name"], "shell_command");
    }

    #[test]
    fn openai_chat_request_adapter_preserves_allowed_tool_choice_for_responses() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "choose an allowed tool"}],
            "tools": [
                {
                    "type": "function",
                    "function": {"name": "lookup_weather", "parameters": {"type": "object"}}
                },
                {
                    "type": "custom",
                    "custom": {"name": "shell_command", "format": {"type": "text"}}
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        {"type": "function", "function": {"name": "lookup_weather"}},
                        {"type": "custom", "custom": {"name": "shell_command"}}
                    ]
                }
            }
        });

        let canonical = from_openai_chat_to_canonical_request(&request).expect("canonical request");
        assert!(canonical.tool_choice.is_none());

        let rebuilt = canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(rebuilt["tool_choice"], request["tool_choice"]);

        let responses = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(responses["tool_choice"]["type"], "allowed_tools");
        assert_eq!(responses["tool_choice"]["mode"], "required");
        assert_eq!(responses["tool_choice"]["tools"][0]["type"], "function");
        assert_eq!(
            responses["tool_choice"]["tools"][0]["name"],
            "lookup_weather"
        );
        assert_eq!(responses["tool_choice"]["tools"][1]["type"], "custom");
        assert_eq!(
            responses["tool_choice"]["tools"][1]["name"],
            "shell_command"
        );
    }

    #[test]
    fn canonical_response_roundtrips_reasoning_tool_usage_and_unknown_extensions() {
        let response = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "reasoning_content": "thinking",
                    "content": [{"type": "text", "text": "done"}],
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "lookup", "arguments": "{\"q\":\"rust\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 5,
                "total_tokens": 8,
                "prompt_tokens_details": {"cached_tokens": 2},
                "completion_tokens_details": {"reasoning_tokens": 1}
            },
            "service_tier": "default"
        });

        let canonical =
            from_openai_chat_to_canonical_response(&response).expect("canonical response");
        assert_eq!(canonical.id, "chatcmpl_1");
        assert!(
            matches!(canonical.content[0], CanonicalContentBlock::Thinking { ref text, .. } if text == "thinking")
        );
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::ToolUse { name, .. } if name == "lookup")
        ));
        assert_eq!(canonical.usage.as_ref().unwrap().cache_read_tokens, 2);
        assert_eq!(canonical.usage.as_ref().unwrap().reasoning_tokens, 1);
        assert_eq!(canonical_response_unknown_block_count(&canonical), 0);
        assert!(canonical.extensions.contains_key("openai"));

        let encoded = serde_json::to_value(&canonical).expect("serialize");
        let decoded =
            serde_json::from_value::<super::CanonicalResponse>(encoded).expect("deserialize");
        assert_eq!(decoded, canonical);

        let rebuilt = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            rebuilt["choices"][0]["message"]["reasoning_content"],
            "thinking"
        );
        assert_eq!(
            rebuilt["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        assert_eq!(
            rebuilt["usage"]["completion_tokens_details"]["reasoning_tokens"],
            1
        );
    }

    #[test]
    fn openai_chat_response_preserves_custom_tool_calls_for_responses() {
        let response = json!({
            "id": "chatcmpl_custom",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_custom_1",
                        "type": "custom",
                        "custom": {
                            "name": "shell_command",
                            "input": "ls -la"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let canonical =
            from_openai_chat_to_canonical_response(&response).expect("canonical response");
        assert!(matches!(
            canonical.content[0],
            CanonicalContentBlock::ToolUse { ref id, ref name, ref input, .. }
                if id == "call_custom_1" && name == "shell_command" && input == "ls -la"
        ));

        let rebuilt_chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["tool_calls"][0]["type"],
            "custom"
        );
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["tool_calls"][0]["custom"]["name"],
            "shell_command"
        );

        let responses = canonical_to_openai_responses_response(&canonical, &json!({}));
        assert_eq!(responses["output"][0]["type"], "custom_tool_call");
        assert_eq!(responses["output"][0]["name"], "shell_command");
        assert_eq!(responses["output"][0]["input"], "ls -la");
    }

    #[test]
    fn openai_responses_response_preserves_custom_tool_calls_for_chat() {
        let response = json!({
            "id": "resp_custom",
            "object": "response",
            "status": "completed",
            "model": "gpt-5",
            "output": [{
                "type": "custom_tool_call",
                "id": "call_custom_1",
                "call_id": "call_custom_1",
                "status": "completed",
                "name": "shell_command",
                "input": "pwd"
            }]
        });

        let canonical =
            from_openai_responses_to_canonical_response(&response).expect("canonical response");
        assert!(matches!(
            canonical.content[0],
            CanonicalContentBlock::ToolUse { ref id, ref name, ref input, .. }
                if id == "call_custom_1" && name == "shell_command" && input == "pwd"
        ));

        let chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["type"],
            "custom"
        );
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["custom"]["name"],
            "shell_command"
        );
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["custom"]["input"],
            "pwd"
        );

        let rebuilt = canonical_to_openai_responses_response(&canonical, &json!({}));
        assert_eq!(rebuilt["output"][0]["type"], "custom_tool_call");
        assert_eq!(rebuilt["output"][0]["input"], "pwd");
    }

    #[test]
    fn openai_response_adapters_preserve_audio_output_shapes() {
        let chat_response = json!({
            "id": "chatcmpl_audio",
            "object": "chat.completion",
            "model": "gpt-5-audio",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello",
                    "audio": {
                        "id": "audio_1",
                        "data": "QUJDRA==",
                        "expires_at": 123,
                        "transcript": "hello"
                    }
                },
                "finish_reason": "stop"
            }]
        });

        let canonical =
            from_openai_chat_to_canonical_response(&chat_response).expect("canonical response");
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::Audio { data, .. } if data.as_deref() == Some("QUJDRA=="))
        ));

        let rebuilt_chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["audio"]["id"],
            "audio_1"
        );
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["audio"]["transcript"],
            "hello"
        );

        let responses = canonical_to_openai_responses_response(&canonical, &json!({}));
        assert_eq!(responses["output"][0]["content"][1]["type"], "output_audio");
        assert_eq!(responses["output"][0]["content"][1]["data"], "QUJDRA==");
        assert_eq!(responses["output"][0]["content"][1]["transcript"], "hello");

        let responses_response = json!({
            "id": "resp_audio",
            "object": "response",
            "status": "completed",
            "model": "gpt-5-audio",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_audio",
                    "data": "RUZHSA==",
                    "transcript": "hi"
                }]
            }]
        });

        let canonical = from_openai_responses_to_canonical_response(&responses_response)
            .expect("canonical response");
        let chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(chat["choices"][0]["message"]["audio"]["data"], "RUZHSA==");
        assert_eq!(chat["choices"][0]["message"]["audio"]["transcript"], "hi");
    }

    #[test]
    fn canonical_request_adapter_keeps_existing_simple_chat_shape() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}],
            "stop": ["x", "y"],
            "n": 2
        });
        let canonical = from_openai_chat_to_canonical_request(&request).expect("canonical request");
        let rebuilt = canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(rebuilt["model"], request["model"]);
        assert_eq!(rebuilt["messages"], request["messages"]);
        assert_eq!(rebuilt["stop"], Value::Array(vec![json!("x"), json!("y")]));
        assert_eq!(rebuilt["n"], 2);
    }

    #[test]
    fn openai_chat_request_adapter_preserves_reasoning_content_for_responses() {
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "user", "content": "hi"},
                {
                    "role": "assistant",
                    "reasoning_content": "internal plan",
                    "content": "final answer"
                }
            ]
        });

        let canonical = from_openai_chat_to_canonical_request(&request).expect("canonical request");
        assert!(matches!(
            canonical.messages[1].content.first(),
            Some(CanonicalContentBlock::Thinking { text, .. }) if text == "internal plan"
        ));

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        let parts = rebuilt["input"][1]["content"]
            .as_array()
            .expect("content parts");

        assert_eq!(parts[0]["type"], "output_text");
        assert!(parts[0]["text"]
            .as_str()
            .expect("reasoning text")
            .contains("<thinking>internal plan</thinking>"));
        assert_eq!(parts[1]["type"], "output_text");
        assert_eq!(parts[1]["text"], "final answer");
    }

    #[test]
    fn openai_responses_request_adapter_preserves_audio_reasoning_tools_and_text_config() {
        let request = json!({
            "model": "gpt-5",
            "instructions": "Be exact.",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "transcribe"},
                        {
                            "type": "input_audio",
                            "input_audio": {
                                "data": "AAAA",
                                "format": "mp3"
                            }
                        }
                    ]
                },
                {
                    "type": "function_call",
                    "call_id": "call_123",
                    "name": "lookup",
                    "arguments": "{\"q\":\"audio\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_123",
                    "output": "{\"ok\":true}"
                }
            ],
            "reasoning": {"effort": "high"},
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "answer",
                    "schema": {"type": "object"},
                    "strict": true
                },
                "verbosity": "low"
            },
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {"type": "object"}
            }],
            "tool_choice": {"type": "function", "name": "lookup"}
        });

        let canonical =
            from_openai_responses_to_canonical_request(&request).expect("canonical request");
        assert_eq!(canonical.instructions[0].text, "Be exact.");
        assert!(matches!(
            canonical.messages[0].content[1],
            CanonicalContentBlock::Audio {
                ref data,
                ref format,
                ..
            } if data.as_deref() == Some("AAAA") && format.as_deref() == Some("mp3")
        ));
        assert!(matches!(
            canonical.messages[1].content[0],
            CanonicalContentBlock::ToolUse { ref id, ref name, .. }
                if id == "call_123" && name == "lookup"
        ));
        assert_eq!(
            canonical
                .thinking
                .as_ref()
                .and_then(|thinking| thinking.extensions.get("openai_responses"))
                .and_then(|value| value.get("effort"))
                .and_then(Value::as_str),
            Some("high")
        );
        assert_eq!(
            canonical
                .response_format
                .as_ref()
                .and_then(|format| format.json_schema.as_ref())
                .and_then(|schema| schema.get("name"))
                .and_then(Value::as_str),
            Some("answer")
        );
        assert_eq!(
            canonical
                .response_format
                .as_ref()
                .and_then(|format| format.json_schema.as_ref())
                .and_then(|schema| schema.get("strict"))
                .and_then(Value::as_bool),
            Some(true)
        );

        let rebuilt_chat =
            canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(rebuilt_chat["response_format"]["type"], "json_schema");
        assert_eq!(
            rebuilt_chat["response_format"]["json_schema"]["name"],
            "answer"
        );

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["model"], "gpt-5-upstream");
        assert_eq!(rebuilt["text"]["format"]["name"], "answer");
        assert_eq!(rebuilt["text"]["format"]["strict"], true);
        assert!(rebuilt["text"]["format"].get("json_schema").is_none());
        assert_eq!(rebuilt["text"]["verbosity"], "low");
        assert_eq!(rebuilt["tool_choice"]["name"], "lookup");
    }

    #[test]
    fn openai_responses_request_adapter_accepts_nested_function_and_custom_tools() {
        let request = json!({
            "model": "gpt-5",
            "input": "Use a tool",
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "description": "Lookup weather",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "city": {"type": "string"}
                            },
                            "required": ["city"]
                        }
                    }
                },
                {
                    "type": "custom",
                    "custom": {
                        "name": "shell_command",
                        "description": "Run a shell command"
                    }
                }
            ],
            "tool_choice": {
                "type": "function",
                "function": {"name": "lookup_weather"}
            }
        });

        let canonical =
            from_openai_responses_to_canonical_request(&request).expect("canonical request");
        assert_eq!(canonical.tools.len(), 2);
        assert_eq!(canonical.tools[0].name, "lookup_weather");
        assert_eq!(
            canonical.tools[0]
                .parameters
                .as_ref()
                .and_then(|value| value.get("required"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(canonical.tools[1].name, "shell_command");
        assert!(matches!(
            canonical.tool_choice,
            Some(super::CanonicalToolChoice::Tool { ref name }) if name == "lookup_weather"
        ));

        let claude = canonical_to_claude_request(&canonical, "claude-sonnet-4-upstream", false)
            .expect("claude request");
        assert_eq!(claude["tools"][0]["name"], "lookup_weather");
        assert_eq!(claude["tools"][1]["name"], "shell_command");
        assert_eq!(claude["tool_choice"]["name"], "lookup_weather");

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["tools"][0]["name"], "lookup_weather");
        assert_eq!(rebuilt["tools"][1]["type"], "custom");
        assert_eq!(rebuilt["tools"][1]["custom"]["name"], "shell_command");
    }

    #[test]
    fn openai_responses_request_adapter_preserves_custom_tool_choice_for_chat() {
        let request = json!({
            "model": "gpt-5",
            "input": "Use the custom tool",
            "tools": [{
                "type": "custom",
                "name": "shell_command",
                "description": "Run a shell command",
                "format": {"type": "text"}
            }],
            "tool_choice": {
                "type": "custom",
                "name": "shell_command"
            }
        });

        let canonical =
            from_openai_responses_to_canonical_request(&request).expect("canonical request");
        assert!(matches!(
            canonical.tool_choice,
            Some(super::CanonicalToolChoice::Tool { ref name }) if name == "shell_command"
        ));

        let chat = canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(chat["tools"][0]["type"], "custom");
        assert_eq!(chat["tools"][0]["custom"]["name"], "shell_command");
        assert_eq!(chat["tools"][0]["custom"]["format"]["type"], "text");
        assert_eq!(chat["tool_choice"]["type"], "custom");
        assert_eq!(chat["tool_choice"]["custom"]["name"], "shell_command");

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["tools"][0]["type"], "custom");
        assert_eq!(rebuilt["tools"][0]["name"], "shell_command");
        assert_eq!(rebuilt["tool_choice"]["type"], "custom");
        assert_eq!(rebuilt["tool_choice"]["name"], "shell_command");
    }

    #[test]
    fn openai_responses_request_adapter_accepts_single_input_item_object() {
        let request = json!({
            "model": "gpt-5",
            "input": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "hello"}]
            }
        });

        let canonical =
            from_openai_responses_to_canonical_request(&request).expect("canonical request");
        assert_eq!(canonical.messages.len(), 1);
        assert_eq!(canonical.messages[0].role, CanonicalRole::User);

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["input"][0]["type"], "message");
        assert_eq!(rebuilt["input"][0]["role"], "user");
        assert_eq!(rebuilt["input"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn openai_responses_request_adapter_preserves_custom_tool_history() {
        let request = json!({
            "model": "gpt-5",
            "input": [
                {
                    "type": "custom_tool_call",
                    "id": "ctc_1",
                    "call_id": "call_custom_1",
                    "name": "shell_command",
                    "input": "ls -la",
                    "status": "completed"
                },
                {
                    "type": "custom_tool_call_output",
                    "call_id": "call_custom_1",
                    "output": "ok"
                }
            ]
        });

        let canonical =
            from_openai_responses_to_canonical_request(&request).expect("canonical request");

        let chat = canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(chat["messages"][0]["tool_calls"][0]["type"], "custom");
        assert_eq!(
            chat["messages"][0]["tool_calls"][0]["custom"]["name"],
            "shell_command"
        );
        assert_eq!(
            chat["messages"][0]["tool_calls"][0]["custom"]["input"],
            "ls -la"
        );

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["input"][0]["type"], "custom_tool_call");
        assert_eq!(rebuilt["input"][0]["id"], "ctc_1");
        assert_eq!(rebuilt["input"][0]["call_id"], "call_custom_1");
        assert_eq!(rebuilt["input"][0]["name"], "shell_command");
        assert_eq!(rebuilt["input"][0]["input"], "ls -la");
        assert_eq!(rebuilt["input"][1]["type"], "custom_tool_call_output");
        assert_eq!(rebuilt["input"][1]["output"], "ok");
    }

    #[test]
    fn openai_responses_request_adapter_preserves_reasoning_encrypted_content_history() {
        let request = json!({
            "model": "gpt-5",
            "input": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "think"}],
                    "encrypted_content": "enc_reasoning"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "done"}]
                }
            ]
        });

        let canonical =
            from_openai_responses_to_canonical_request(&request).expect("canonical request");
        assert!(matches!(
            canonical.messages[0].content[0],
            CanonicalContentBlock::Thinking { ref text, ref encrypted_content, .. }
                if text == "think" && encrypted_content.as_deref() == Some("enc_reasoning")
        ));

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["input"][0]["type"], "reasoning");
        assert_eq!(rebuilt["input"][0]["summary"][0]["text"], "think");
        assert_eq!(rebuilt["input"][0]["encrypted_content"], "enc_reasoning");
        assert_eq!(rebuilt["input"][1]["type"], "message");
        assert_eq!(rebuilt["input"][1]["content"][0]["text"], "done");
    }

    #[test]
    fn openai_responses_request_adapter_preserves_file_url_and_tool_output_parts() {
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "user", "content": [
                    {"type": "file", "file": {"file_url": "https://example.com/input.pdf", "filename": "input.pdf"}}
                ]},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_render",
                    "type": "function",
                    "function": {"name": "render", "arguments": "{}"}
                }]},
                {"role": "tool", "tool_call_id": "call_render", "content": [
                    {"type": "text", "text": "Rendered result attached."},
                    {"type": "image_url", "image_url": {"url": "https://example.com/out.png", "detail": "high"}},
                    {"type": "file", "file": {"file_url": "https://example.com/report.pdf", "filename": "report.pdf"}}
                ]}
            ]
        });

        let canonical = from_openai_chat_to_canonical_request(&request).expect("canonical request");
        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");

        assert_eq!(
            rebuilt["input"][0]["content"][0]["file_url"],
            "https://example.com/input.pdf"
        );
        assert!(rebuilt["input"][0]["content"][0].get("file_data").is_none());
        assert_eq!(rebuilt["input"][2]["type"], "function_call_output");
        assert_eq!(rebuilt["input"][2]["output"][0]["type"], "input_text");
        assert_eq!(
            rebuilt["input"][2]["output"][1]["image_url"],
            "https://example.com/out.png"
        );
        assert_eq!(rebuilt["input"][2]["output"][1]["detail"], "high");
        assert_eq!(
            rebuilt["input"][2]["output"][2]["file_url"],
            "https://example.com/report.pdf"
        );
    }

    #[test]
    fn openai_responses_request_adapter_preserves_allowed_tool_choice_for_chat() {
        let request = json!({
            "model": "gpt-5",
            "input": "choose an allowed tool",
            "tools": [
                {"type": "function", "name": "lookup_weather", "parameters": {"type": "object"}},
                {"type": "custom", "name": "shell_command", "format": {"type": "text"}}
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "auto",
                "tools": [
                    {"type": "function", "name": "lookup_weather"},
                    {"type": "custom", "name": "shell_command"}
                ]
            }
        });

        let canonical =
            from_openai_responses_to_canonical_request(&request).expect("canonical request");
        assert!(canonical.tool_choice.is_none());

        let chat = canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(chat["tool_choice"]["type"], "allowed_tools");
        assert_eq!(chat["tool_choice"]["allowed_tools"]["mode"], "auto");
        assert_eq!(
            chat["tool_choice"]["allowed_tools"]["tools"][0]["function"]["name"],
            "lookup_weather"
        );
        assert_eq!(
            chat["tool_choice"]["allowed_tools"]["tools"][1]["custom"]["name"],
            "shell_command"
        );

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["tool_choice"], request["tool_choice"]);
    }

    #[test]
    fn openai_responses_response_adapter_preserves_output_items_reasoning_tools_and_usage() {
        let response = json!({
            "id": "resp_123",
            "object": "response",
            "status": "completed",
            "error": null,
            "model": "gpt-5",
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{
                        "type": "summary_text",
                        "text": "think"
                    }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "done",
                            "annotations": [{
                                "type": "file_citation",
                                "start_index": 1,
                                "end_index": 3
                            }]
                        },
                        {"type": "refusal", "refusal": "partial refusal"},
                        {
                            "type": "output_image",
                            "image_url": "data:image/png;base64,iVBORw0KGgo="
                        },
                        {
                            "type": "file",
                            "file": {
                                "file_data": "data:application/pdf;base64,JVBERi0x",
                                "filename": "report.pdf"
                            }
                        }
                    ]
                },
                {
                    "type": "function_call",
                    "id": "call_1",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{\"q\":\"rust\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": {"ok": true}
                },
                {
                    "type": "local_shell_call",
                    "id": "lsc_1",
                    "call_id": "call_shell_1",
                    "status": "completed",
                    "action": {
                        "type": "exec",
                        "command": ["pwd"]
                    }
                },
                {
                    "type": "local_shell_call_output",
                    "call_id": "call_shell_1",
                    "output": {
                        "stdout": "/tmp/project\n",
                        "stderr": "",
                        "outcome": "success"
                    }
                },
                {
                    "type": "future_item",
                    "payload": true
                }
            ],
            "usage": {
                "input_tokens": 3,
                "input_tokens_details": {
                    "cache_write_tokens": 1,
                    "cached_tokens": 2
                },
                "output_tokens": 5,
                "output_tokens_details": {"reasoning_tokens": 1},
                "total_tokens": 8
            },
            "service_tier": "flex"
        });

        let canonical =
            from_openai_responses_to_canonical_response(&response).expect("canonical response");
        assert_eq!(canonical.id, "resp_123");
        assert!(matches!(
            canonical.content[0],
            CanonicalContentBlock::Thinking { ref text, .. } if text == "think"
        ));
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::ToolUse { name, .. } if name == "lookup")
        ));
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::ToolUse { name, input, .. }
                if name == "local_shell" && input["action"]["command"][0] == "pwd")
        ));
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1")
        ));
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::ToolResult { tool_use_id, name: Some(name), .. }
                if tool_use_id == "call_shell_1" && name == "local_shell")
        ));
        assert_eq!(canonical_response_unknown_block_count(&canonical), 2);
        assert_eq!(canonical.usage.as_ref().unwrap().cache_read_tokens, 2);
        assert_eq!(canonical.usage.as_ref().unwrap().cache_write_tokens, 1);
        assert_eq!(canonical.usage.as_ref().unwrap().reasoning_tokens, 1);

        let rebuilt_chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["annotations"],
            json!([{"type": "file_citation", "start_index": 1, "end_index": 3}])
        );
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["refusal"],
            "partial refusal"
        );
        assert_eq!(rebuilt_chat["service_tier"], "flex");

        let rebuilt = canonical_to_openai_responses_response(&canonical, &json!({}));
        assert_eq!(rebuilt["id"], "resp_123");
        assert_eq!(rebuilt["output"][0]["type"], "reasoning");
        assert_eq!(rebuilt["output"][1]["content"][0]["text"], "done");
        assert_eq!(rebuilt["output"][1]["content"][1]["type"], "refusal");
        assert_eq!(rebuilt["output"][2]["type"], "function_call");
        assert_eq!(rebuilt["output"][3]["type"], "function_call_output");
        assert_eq!(rebuilt["output"][4]["type"], "local_shell_call");
        assert_eq!(rebuilt["output"][4]["action"]["command"][0], "pwd");
        assert_eq!(rebuilt["output"][5]["type"], "local_shell_call_output");
        assert_eq!(rebuilt["output"][5]["call_id"], "call_shell_1");
        assert_eq!(rebuilt["usage"]["input_tokens_details"]["cached_tokens"], 2);
        assert_eq!(
            rebuilt["usage"]["input_tokens_details"]["cache_write_tokens"],
            1
        );
        assert_eq!(
            rebuilt["usage"]["output_tokens_details"]["reasoning_tokens"],
            1
        );
        assert_eq!(rebuilt["service_tier"], "flex");
    }

    #[test]
    fn openai_responses_to_claude_response_drops_empty_pages_only_for_read_tool() {
        let response = json!({
            "id": "resp_read_pages",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.5",
            "output": [
                {
                    "type": "function_call",
                    "id": "call_read",
                    "call_id": "call_read",
                    "name": "Read",
                    "arguments": "{\"file_path\":\"/tmp/a.txt\",\"offset\":0,\"limit\":20,\"pages\":\"\"}"
                },
                {
                    "type": "function_call",
                    "id": "call_search",
                    "call_id": "call_search",
                    "name": "Search",
                    "arguments": "{\"query\":\"\",\"pages\":\"\"}"
                }
            ],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1,
                "total_tokens": 2
            }
        });

        let canonical =
            from_openai_responses_to_canonical_response(&response).expect("canonical response");
        let claude = canonical_to_claude_response(&canonical);

        assert_eq!(
            claude["content"][0]["input"],
            json!({
                "file_path": "/tmp/a.txt",
                "offset": 0,
                "limit": 20,
            })
        );
        assert_eq!(
            claude["content"][1]["input"],
            json!({
                "query": "",
                "pages": "",
            })
        );

        let rebuilt_responses = canonical_to_openai_responses_response(&canonical, &json!({}));
        let read_arguments = serde_json::from_str::<Value>(
            rebuilt_responses["output"][0]["arguments"]
                .as_str()
                .expect("arguments should be a string"),
        )
        .expect("arguments should be json");
        assert_eq!(
            read_arguments,
            json!({
                "file_path": "/tmp/a.txt",
                "offset": 0,
                "limit": 20,
                "pages": "",
            })
        );
    }

    #[test]
    fn openai_responses_image_generation_call_becomes_canonical_image_block() {
        let response = json!({
            "id": "resp_img",
            "model": "gpt-image-2",
            "status": "completed",
            "output": [{
                "id": "ig_1",
                "type": "image_generation_call",
                "status": "completed",
                "output_format": "png",
                "result": "aW1hZ2U="
            }]
        });

        let canonical =
            from_openai_responses_to_canonical_response(&response).expect("canonical response");
        assert!(matches!(
            canonical.content[0],
            CanonicalContentBlock::Image { ref data, ref media_type, .. }
                if data.as_deref() == Some("aW1hZ2U=")
                    && media_type.as_deref() == Some("image/png")
        ));

        let rebuilt_chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["content"][0]["type"],
            json!("image_url")
        );
        assert_eq!(
            rebuilt_chat["choices"][0]["message"]["content"][0]["image_url"]["url"],
            json!("data:image/png;base64,aW1hZ2U=")
        );

        let rebuilt_responses = canonical_to_openai_responses_response(&canonical, &json!({}));
        assert_eq!(
            rebuilt_responses["output"][0]["type"],
            json!("image_generation_call")
        );
        assert_eq!(rebuilt_responses["output"][0]["result"], json!("aW1hZ2U="));
    }

    #[test]
    fn claude_request_adapter_preserves_cache_thinking_tools_and_builtin_extensions() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "system": [
                {
                    "type": "text",
                    "text": "Cache this.",
                    "cache_control": {"type": "ephemeral"}
                },
                {"type": "text", "text": "Be exact."}
            ],
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "plan",
                            "signature": "sig_123"
                        },
                        {
                            "type": "tool_use",
                            "name": "lookup",
                            "input": {"q": "rust"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_auto_0",
                        "content": {"ok": true}
                    }]
                }
            ],
            "tools": [
                {
                    "name": "lookup",
                    "description": "Lookup",
                    "input_schema": {"type": "object"}
                },
                {
                    "type": "web_search_20250305",
                    "name": "web_search",
                    "max_uses": 5
                }
            ],
            "tool_choice": {
                "type": "auto",
                "disable_parallel_tool_use": false
            },
            "thinking": {"type": "enabled", "budget_tokens": 2048},
            "output_config": {"effort": "medium"}
        });

        let canonical = from_claude_to_canonical_request(&request).expect("canonical request");
        assert_eq!(canonical.instructions.len(), 2);
        assert_eq!(
            canonical.instructions[0]
                .extensions
                .get("claude")
                .and_then(|value| value.get("cache_control"))
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str),
            Some("ephemeral")
        );
        assert!(matches!(
            canonical.messages[0].content[0],
            CanonicalContentBlock::Thinking {
                ref text,
                ref signature,
                ..
            } if text == "plan" && signature.as_deref() == Some("sig_123")
        ));
        assert!(matches!(
            canonical.messages[0].content[1],
            CanonicalContentBlock::ToolUse { ref id, .. } if id == "toolu_auto_0"
        ));

        let openai_chat =
            canonical_to_openai_chat_request(&canonical).expect("openai chat request");
        assert_eq!(
            openai_chat["messages"][2]["reasoning_parts"][0]["signature"],
            "sig_123"
        );
        assert_eq!(
            openai_chat["web_search_options"]["search_context_size"],
            "medium"
        );

        let rebuilt =
            canonical_to_claude_request(&canonical, "claude-upstream", false).expect("claude");
        assert_eq!(rebuilt["model"], "claude-upstream");
        assert_eq!(rebuilt["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(rebuilt["messages"][1]["content"][0]["signature"], "sig_123");
        assert_eq!(rebuilt["tools"][1]["type"], "web_search_20250305");
        assert_eq!(rebuilt["thinking"]["budget_tokens"], 2048);
        assert_eq!(rebuilt["output_config"]["effort"], "medium");
    }

    #[test]
    fn claude_request_adapter_preserves_official_raw_content_blocks() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "messages": [{
                "role": "assistant",
                "content": [
                    {
                        "type": "server_tool_use",
                        "id": "srvu_1",
                        "name": "web_search",
                        "input": {"query": "rust"}
                    },
                    {
                        "type": "web_search_tool_result",
                        "tool_use_id": "srvu_1",
                        "content": [{
                            "type": "web_search_result",
                            "title": "Rust",
                            "url": "https://www.rust-lang.org/",
                            "encrypted_content": "enc"
                        }]
                    }
                ]
            }]
        });

        let canonical = from_claude_to_canonical_request(&request).expect("canonical request");
        assert_eq!(canonical_request_unknown_block_count(&canonical), 2);

        let rebuilt = canonical_to_claude_request(&canonical, "claude-upstream", false)
            .expect("claude request");
        assert_eq!(
            rebuilt["messages"][1]["content"][0]["type"],
            "server_tool_use"
        );
        assert_eq!(
            rebuilt["messages"][1]["content"][1]["type"],
            "web_search_tool_result"
        );
        assert_eq!(
            rebuilt["messages"][1]["content"][1]["content"][0]["encrypted_content"],
            "enc"
        );
    }

    #[test]
    fn canonical_to_openai_chat_request_rejects_unrepresentable_claude_tool_result() {
        let request = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_read",
                    "content": [{
                        "type": "image",
                        "source": {
                            "type": "unsupported",
                            "media_type": "image/png",
                            "data": "AAAA"
                        }
                    }]
                }]
            }]
        });

        let canonical = from_claude_to_canonical_request(&request).expect("canonical request");

        assert!(canonical_to_openai_chat_request(&canonical).is_none());
    }

    #[test]
    fn claude_response_adapter_preserves_thinking_signature_tool_and_cache_usage() {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [
                {
                    "type": "thinking",
                    "thinking": "plan",
                    "signature": "sig_123"
                },
                {"type": "text", "text": "done"},
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "lookup",
                    "input": {"q": "rust"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "cache_read_input_tokens": 3,
                "cache_creation_input_tokens": 2,
                "output_tokens_details": {"thinking_tokens": 4}
            }
        });

        let canonical = from_claude_to_canonical_response(&response).expect("canonical response");
        assert!(matches!(
            canonical.content[0],
            CanonicalContentBlock::Thinking {
                ref text,
                ref signature,
                ..
            } if text == "plan" && signature.as_deref() == Some("sig_123")
        ));
        assert!(matches!(
            canonical.content[2],
            CanonicalContentBlock::ToolUse { ref id, ref name, .. }
                if id == "toolu_123" && name == "lookup"
        ));
        assert_eq!(canonical.usage.as_ref().unwrap().cache_read_tokens, 3);
        assert_eq!(canonical.usage.as_ref().unwrap().cache_write_tokens, 2);
        assert_eq!(canonical.usage.as_ref().unwrap().reasoning_tokens, 4);

        let rebuilt = canonical_to_claude_response(&canonical);
        assert_eq!(rebuilt["content"][0]["signature"], "sig_123");
        assert_eq!(rebuilt["content"][2]["name"], "lookup");
        assert_eq!(rebuilt["stop_reason"], "tool_use");
        assert_eq!(rebuilt["usage"]["cache_read_input_tokens"], 3);
        assert_eq!(rebuilt["usage"]["cache_creation_input_tokens"], 2);
        assert_eq!(
            rebuilt["usage"]["output_tokens_details"]["thinking_tokens"],
            4
        );

        let rebuilt_chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            rebuilt_chat["usage"]["completion_tokens_details"]["reasoning_tokens"],
            4
        );

        let rebuilt_openai = canonical_to_openai_responses_response(&canonical, &json!({}));
        assert_eq!(
            rebuilt_openai["usage"]["output_tokens_details"]["reasoning_tokens"],
            4
        );
        assert_eq!(rebuilt_openai["usage"]["input_tokens"], 16);
        assert_eq!(
            rebuilt_openai["usage"]["input_tokens_details"]["cached_tokens"],
            3
        );
        assert_eq!(
            rebuilt_openai["usage"]["input_tokens_details"]["cache_write_tokens"],
            2
        );
        assert_eq!(rebuilt_openai["usage"]["total_tokens"], 23);

        let rebuilt_gemini = canonical_to_gemini_response(&canonical, &json!({})).expect("gemini");
        assert_eq!(rebuilt_gemini["usageMetadata"]["promptTokenCount"], 16);
        assert_eq!(
            rebuilt_gemini["usageMetadata"]["cachedContentTokenCount"],
            3
        );
        assert_eq!(rebuilt_gemini["usageMetadata"]["totalTokenCount"], 23);
    }

    #[test]
    fn openai_usage_prefers_cache_write_tokens_and_emits_the_official_field() {
        let response = json!({
            "id": "resp_cache_write",
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [],
            "usage": {
                "input_tokens": 20,
                "output_tokens": 4,
                "total_tokens": 24,
                "input_tokens_details": {
                    "cached_tokens": 3,
                    "cache_write_tokens": 7,
                    "cached_creation_tokens": 99
                }
            }
        });

        let canonical = from_openai_responses_to_canonical_response(&response)
            .expect("Responses usage should parse");
        assert_eq!(canonical.usage.as_ref().unwrap().cache_write_tokens, 7);

        let rebuilt = canonical_to_openai_responses_response(&canonical, &json!({}));
        assert_eq!(
            rebuilt["usage"]["input_tokens_details"]["cache_write_tokens"],
            7
        );
        assert!(rebuilt["usage"]["input_tokens_details"]
            .get("cached_creation_tokens")
            .is_none());

        let rebuilt_chat = canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            rebuilt_chat["usage"]["prompt_tokens_details"]["cache_write_tokens"],
            7
        );
        assert!(rebuilt_chat["usage"]["prompt_tokens_details"]
            .get("cached_creation_tokens")
            .is_none());
    }

    #[test]
    fn openai_usage_accepts_cached_creation_tokens_as_legacy_input_alias() {
        let response = json!({
            "id": "resp_cache_write_legacy",
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 2,
                "input_tokens_details": {"cached_creation_tokens": 5}
            }
        });

        let canonical = from_openai_responses_to_canonical_response(&response)
            .expect("legacy usage alias should parse");
        assert_eq!(canonical.usage.as_ref().unwrap().cache_write_tokens, 5);
    }

    #[test]
    fn gemini_request_adapter_preserves_thinking_tools_media_and_extensions() {
        let request = json!({
            "systemInstruction": {
                "parts": [
                    {"text": "Be exact.", "cacheControl": {"type": "ephemeral"}}
                ]
            },
            "contents": [
                {
                    "role": "user",
                    "parts": [
                        {"text": "Inspect this"},
                        {"inlineData": {"mimeType": "image/png", "data": "iVBORw0KGgo="}},
                        {"fileData": {"fileUri": "https://example.com/spec.pdf", "mimeType": "application/pdf"}}
                    ]
                },
                {
                    "role": "model",
                    "parts": [
                        {"text": "plan", "thought": true, "thoughtSignature": "sig_123"},
                        {"functionCall": {"id": "call_123", "name": "lookup", "args": {"q": "rust"}}}
                    ]
                },
                {
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "id": "call_123",
                            "name": "lookup",
                            "response": {"result": {"ok": true}}
                        }
                    }]
                }
            ],
            "generationConfig": {
                "maxOutputTokens": 64,
                "temperature": 0.2,
                "topP": 0.9,
                "topK": 40,
                "candidateCount": 2,
                "seed": 7,
                "stopSequences": ["END"],
                "thinkingConfig": {"includeThoughts": true, "thinkingBudget": 2048},
                "responseMimeType": "application/json",
                "responseSchema": {"type": "object"},
                "responseModalities": ["TEXT"],
                "routingConfig": {"autoMode": {}}
            },
            "tools": [
                {"googleSearch": {}},
                {"codeExecution": {}},
                {
                    "functionDeclarations": [{
                        "name": "lookup",
                        "description": "Lookup data",
                        "parameters": {"type": "object"}
                    }]
                }
            ],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup"]
                }
            },
            "safetySettings": [{"category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE"}],
            "cachedContent": "cached/abc"
        });

        let canonical = from_gemini_to_canonical_request(
            &request,
            "/v1beta/models/gemini-2.5-pro:generateContent",
        )
        .expect("canonical request");

        assert_eq!(canonical.model, "gemini-2.5-pro");
        assert_eq!(canonical.instructions[0].text, "Be exact.");
        assert_eq!(canonical.generation.max_tokens, Some(64));
        assert_eq!(canonical.generation.top_k, Some(40));
        assert_eq!(canonical.generation.n, Some(2));
        assert_eq!(canonical.tools[0].name, "lookup");
        assert!(matches!(
            canonical.messages[0].content[1],
            CanonicalContentBlock::Image {
                ref data,
                ref media_type,
                ..
            } if data.as_deref() == Some("iVBORw0KGgo=")
                && media_type.as_deref() == Some("image/png")
        ));
        assert!(matches!(
            canonical.messages[1].content[0],
            CanonicalContentBlock::Thinking {
                ref text,
                ref signature,
                ..
            } if text == "plan" && signature.as_deref() == Some("sig_123")
        ));
        assert_eq!(
            canonical
                .extensions
                .get("gemini")
                .and_then(|value| value.get("cached_content"))
                .and_then(Value::as_str),
            Some("cached/abc")
        );
        assert_eq!(
            canonical
                .extensions
                .get("openai")
                .and_then(|value| value.get("web_search_options")),
            Some(&json!({}))
        );

        let rebuilt =
            canonical_to_gemini_request(&canonical, "gemini-upstream", false).expect("gemini");
        assert_eq!(rebuilt["model"], "gemini-upstream");
        assert_eq!(
            rebuilt["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            2048
        );
        assert_eq!(
            rebuilt["generationConfig"]["responseModalities"],
            json!(["TEXT"])
        );
        assert_eq!(
            rebuilt["generationConfig"]["routingConfig"]["autoMode"],
            json!({})
        );
        assert_eq!(rebuilt["safetySettings"], request["safetySettings"]);
        assert_eq!(rebuilt["cachedContent"], "cached/abc");
        assert_eq!(rebuilt["tools"], request["tools"]);
        assert_eq!(rebuilt["toolConfig"], request["toolConfig"]);
    }

    #[test]
    fn gemini_request_adapter_normalizes_google_search_grounding_aliases() {
        let cases = [
            (
                "current_camel",
                json!({"googleSearch": {"excludeDomains": ["example.com"]}}),
                "googleSearch",
                false,
                json!({"excludeDomains": ["example.com"]}),
                json!({"excludeDomains": ["example.com"]}),
            ),
            (
                "current_snake",
                json!({"google_search": {"exclude_domains": ["example.com"]}}),
                "google_search",
                false,
                json!({"excludeDomains": ["example.com"]}),
                json!({"excludeDomains": ["example.com"]}),
            ),
            (
                "legacy_snake",
                json!({
                    "google_search_retrieval": {
                        "dynamic_retrieval_config": {
                            "mode": "MODE_DYNAMIC",
                            "dynamic_threshold": 0.7
                        }
                    }
                }),
                "google_search_retrieval",
                true,
                json!({
                    "dynamicRetrievalConfig": {
                        "mode": "MODE_DYNAMIC",
                        "dynamicThreshold": 0.7
                    }
                }),
                json!({}),
            ),
            (
                "legacy_camel",
                json!({
                    "googleSearchRetrieval": {
                        "dynamicRetrievalConfig": {
                            "mode": "MODE_DYNAMIC",
                            "dynamicThreshold": 0.7
                        }
                    }
                }),
                "googleSearchRetrieval",
                true,
                json!({
                    "dynamicRetrievalConfig": {
                        "mode": "MODE_DYNAMIC",
                        "dynamicThreshold": 0.7
                    }
                }),
                json!({}),
            ),
        ];

        for (
            name,
            tool,
            source_field,
            legacy,
            expected_extension_payload,
            expected_output_payload,
        ) in cases
        {
            let request = json!({
                "model": "gemini-2.5-pro",
                "contents": [{"role": "user", "parts": [{"text": "search"}]}],
                "tools": [tool]
            });

            let canonical = from_gemini_to_canonical_request(
                &request,
                "/v1beta/models/gemini-2.5-pro:generateContent",
            )
            .unwrap_or_else(|| panic!("{name}: canonical request"));

            assert_eq!(
                canonical
                    .extensions
                    .get("openai")
                    .and_then(|value| value.get("web_search_options")),
                Some(&json!({})),
                "{name}: web search option"
            );
            let google_search = canonical
                .extensions
                .get("gemini")
                .and_then(|value| value.get("grounding"))
                .and_then(|value| value.get("google_search"))
                .unwrap_or_else(|| panic!("{name}: gemini google_search grounding"));
            assert_eq!(
                google_search.get("source_field").and_then(Value::as_str),
                Some(source_field),
                "{name}: source field"
            );
            assert_eq!(
                google_search.get("legacy").and_then(Value::as_bool),
                Some(legacy),
                "{name}: legacy flag"
            );
            assert_eq!(
                google_search.get("payload"),
                Some(&expected_extension_payload),
                "{name}: normalized payload"
            );

            let rebuilt =
                canonical_to_gemini_request(&canonical, "gemini-upstream", false).unwrap();
            assert_eq!(
                rebuilt["tools"],
                json!([{"googleSearch": expected_output_payload}]),
                "{name}: canonical output"
            );
        }
    }

    #[test]
    fn gemini_request_adapter_keeps_agent_search_retrieval_separate_from_google_search() {
        let request = json!({
            "model": "gemini-2.5-pro",
            "contents": [{"role": "user", "parts": [{"text": "private data"}]}],
            "tools": [{
                "retrieval": {
                    "vertexAiSearch": {
                        "datastore": "projects/p/locations/global/collections/default_collection/dataStores/d"
                    }
                }
            }]
        });

        let canonical = from_gemini_to_canonical_request(
            &request,
            "/v1beta/models/gemini-2.5-pro:generateContent",
        )
        .expect("canonical request");

        assert_eq!(
            canonical
                .extensions
                .get("openai")
                .and_then(|value| value.get("web_search_options")),
            None
        );

        let rebuilt = canonical_to_gemini_request(&canonical, "gemini-upstream", false).unwrap();
        assert_eq!(rebuilt["tools"], request["tools"]);
        assert!(rebuilt["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool.get("googleSearch").is_none()));
    }

    #[test]
    fn gemini_request_adapter_preserves_combined_search_builtin_tool_fields() {
        let cases = [
            (
                "current_snake",
                json!({
                    "google_search": {},
                    "code_execution": {},
                    "url_context": {},
                    "retrieval": {
                        "vertexAiSearch": {
                            "datastore": "projects/p/locations/global/collections/default_collection/dataStores/d"
                        }
                    }
                }),
            ),
            (
                "legacy_snake",
                json!({
                    "google_search_retrieval": {
                        "dynamic_retrieval_config": {
                            "mode": "MODE_DYNAMIC",
                            "dynamic_threshold": 0.7
                        }
                    },
                    "code_execution": {},
                    "url_context": {}
                }),
            ),
        ];

        for (name, tool) in cases {
            let request = json!({
                "model": "gemini-2.5-pro",
                "contents": [{"role": "user", "parts": [{"text": "search with builtins"}]}],
                "tools": [tool]
            });

            let canonical = from_gemini_to_canonical_request(
                &request,
                "/v1beta/models/gemini-2.5-pro:generateContent",
            )
            .unwrap_or_else(|| panic!("{name}: canonical request"));

            let rebuilt =
                canonical_to_gemini_request(&canonical, "gemini-upstream", false).unwrap();
            let tools = rebuilt["tools"]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: tools array"));
            assert!(
                tools.iter().any(|tool| tool.get("googleSearch").is_some()),
                "{name}: google search should be preserved"
            );
            assert!(
                tools.iter().any(|tool| tool.get("codeExecution").is_some()),
                "{name}: code execution should be preserved"
            );
            assert!(
                tools.iter().any(|tool| tool.get("urlContext").is_some()),
                "{name}: URL context should be preserved"
            );
            if name == "current_snake" {
                assert!(
                    tools.iter().any(|tool| tool.get("retrieval").is_some()),
                    "{name}: unhandled retrieval should be preserved"
                );
            }
        }
    }

    #[test]
    fn gemini_response_adapter_preserves_thought_signature_tool_and_usage() {
        let response = json!({
            "responseId": "resp_123",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "index": 0,
                "finishReason": "STOP",
                "content": {
                    "parts": [
                        {"text": "plan", "thought": true, "thoughtSignature": "sig_123"},
                        {"text": "done"},
                        {"functionCall": {"id": "call_123", "name": "lookup", "args": {"q": "rust"}}},
                        {
                            "functionResponse": {
                                "id": "call_123",
                                "name": "lookup",
                                "response": {"result": {"ok": true}}
                            }
                        }
                    ]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "cachedContentTokenCount": 4,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 17
            }
        });

        let canonical = from_gemini_to_canonical_response(&response).expect("canonical response");
        assert_eq!(canonical.id, "resp_123");
        assert_eq!(canonical.model, "gemini-2.5-pro");
        assert!(matches!(
            canonical.content[0],
            CanonicalContentBlock::Thinking {
                ref text,
                ref signature,
                ..
            } if text == "plan" && signature.as_deref() == Some("sig_123")
        ));
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::ToolUse { name, .. } if name == "lookup")
        ));
        assert!(canonical.content.iter().any(
            |block| matches!(block, CanonicalContentBlock::ToolResult {
                tool_use_id,
                name: Some(name),
                output: Some(output),
                ..
            } if tool_use_id == "call_123" && name == "lookup" && output == &json!({"ok": true}))
        ));
        assert_eq!(canonical.usage.as_ref().unwrap().input_tokens, 10);
        assert!(canonical.usage.as_ref().unwrap().input_tokens_include_cache);
        assert_eq!(canonical.usage.as_ref().unwrap().cache_read_tokens, 4);
        assert_eq!(canonical.usage.as_ref().unwrap().output_tokens, 7);
        assert_eq!(canonical.usage.as_ref().unwrap().reasoning_tokens, 2);

        let claude = canonical_to_claude_response(&canonical);
        assert_eq!(claude["usage"]["input_tokens"], 6);
        assert_eq!(claude["usage"]["cache_read_input_tokens"], 4);

        let rebuilt = canonical_to_gemini_response(&canonical, &json!({})).expect("gemini");
        assert_eq!(
            rebuilt["candidates"][0]["content"]["parts"][0]["thoughtSignature"],
            "sig_123"
        );
        assert_eq!(
            rebuilt["candidates"][0]["content"]["parts"][2]["functionCall"]["name"],
            "lookup"
        );
        assert_eq!(
            rebuilt["candidates"][0]["content"]["parts"][3]["functionResponse"]["response"],
            json!({"ok": true})
        );
        assert_eq!(rebuilt["usageMetadata"]["promptTokenCount"], 10);
        assert_eq!(rebuilt["usageMetadata"]["cachedContentTokenCount"], 4);
        assert_eq!(rebuilt["usageMetadata"]["totalTokenCount"], 17);
        assert_eq!(rebuilt["usageMetadata"]["thoughtsTokenCount"], 2);
    }

    #[test]
    fn gemini_response_adapter_preserves_grounding_metadata() {
        let grounding_metadata = json!({
            "webSearchQueries": ["query"],
            "searchEntryPoint": {"renderedContent": "<style></style>"},
            "groundingChunks": [{
                "web": {
                    "uri": "https://example.com",
                    "title": "Example"
                }
            }],
            "groundingSupports": []
        });
        let response = json!({
            "responseId": "resp_grounded",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "index": 0,
                "finishReason": "STOP",
                "groundingMetadata": grounding_metadata,
                "content": {
                    "parts": [{"text": "grounded answer"}]
                }
            }]
        });

        let canonical = from_gemini_to_canonical_response(&response).expect("canonical response");
        assert_eq!(
            canonical.outputs[0]
                .extensions
                .get("gemini")
                .and_then(|value| value.get("groundingMetadata")),
            Some(&grounding_metadata)
        );

        let rebuilt = canonical_to_gemini_response(&canonical, &json!({})).expect("gemini");
        assert_eq!(
            rebuilt["candidates"][0]["groundingMetadata"],
            grounding_metadata
        );
    }

    #[test]
    fn canonical_response_preserves_openai_choices_and_gemini_candidates() {
        let openai_response = json!({
            "id": "chatcmpl_multi",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "first"},
                    "finish_reason": "stop"
                },
                {
                    "index": 1,
                    "message": {"role": "assistant", "content": "second"},
                    "finish_reason": "length"
                }
            ],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
        });
        let canonical =
            from_openai_chat_to_canonical_response(&openai_response).expect("canonical");
        assert_eq!(canonical.outputs.len(), 2);
        let rebuilt = canonical_to_openai_chat_response(&canonical);
        assert_eq!(rebuilt["choices"][0]["message"]["content"], "first");
        assert_eq!(rebuilt["choices"][1]["message"]["content"], "second");
        assert_eq!(rebuilt["choices"][1]["finish_reason"], "length");

        let gemini_response = json!({
            "responseId": "gemini_multi",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [
                {
                    "index": 0,
                    "finishReason": "STOP",
                    "content": {"role": "model", "parts": [{"text": "first"}]}
                },
                {
                    "index": 1,
                    "finishReason": "MAX_TOKENS",
                    "content": {"role": "model", "parts": [{"text": "second"}]}
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        });
        let canonical = from_gemini_to_canonical_response(&gemini_response).expect("canonical");
        assert_eq!(canonical.outputs.len(), 2);
        let rebuilt = canonical_to_openai_chat_response(&canonical);
        assert_eq!(rebuilt["choices"][0]["message"]["content"], "first");
        assert_eq!(rebuilt["choices"][1]["message"]["content"], "second");
        assert_eq!(rebuilt["choices"][1]["finish_reason"], "length");
    }
}
