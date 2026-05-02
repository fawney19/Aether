use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::request::openai::map_thinking_budget_to_openai_reasoning_effort;

pub use crate::protocol::stream::{CanonicalStreamEvent, CanonicalStreamFrame};

pub(crate) const OPENAI_RESPONSES_EXTENSION_NAMESPACE: &str = "openai_responses";
pub(crate) const OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE: &str = "openai_cli";

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
    crate::protocol::formats::openai_chat::request::from_raw(body_json)
}

pub fn canonical_to_openai_chat_request(canonical: &CanonicalRequest) -> Value {
    crate::protocol::formats::openai_chat::request::to_raw(canonical)
}

pub fn from_openai_responses_to_canonical_request(body_json: &Value) -> Option<CanonicalRequest> {
    crate::protocol::formats::openai_responses::request::from_raw(body_json)
}

pub(crate) fn canonical_to_openai_responses_request_with_profile(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
    compact: bool,
) -> Option<Value> {
    crate::protocol::formats::openai_responses::request::to_raw(
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
    crate::protocol::formats::claude_messages::request::from_raw(body_json)
}

pub fn canonical_to_claude_request(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    crate::protocol::formats::claude_messages::request::to_raw(
        canonical,
        mapped_model,
        upstream_is_stream,
    )
}

pub fn from_gemini_to_canonical_request(
    body_json: &Value,
    request_path: &str,
) -> Option<CanonicalRequest> {
    crate::protocol::formats::gemini_generate_content::request::from_raw(body_json, request_path)
}

pub fn canonical_to_gemini_request(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    crate::protocol::formats::gemini_generate_content::request::to_raw(
        canonical,
        mapped_model,
        upstream_is_stream,
    )
}

pub fn from_openai_chat_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::protocol::formats::openai_chat::response::from_raw(body_json)
}

pub fn from_openai_responses_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::protocol::formats::openai_responses::response::from_raw(body_json)
}

pub fn from_claude_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::protocol::formats::claude_messages::response::from_raw(body_json)
}

pub fn from_gemini_to_canonical_response(body_json: &Value) -> Option<CanonicalResponse> {
    crate::protocol::formats::gemini_generate_content::response::from_raw(body_json)
}

pub fn canonical_to_openai_chat_response(canonical: &CanonicalResponse) -> Value {
    crate::protocol::formats::openai_chat::response::to_raw(canonical)
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
                id, name, input, ..
            } => {
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": canonicalize_tool_arguments(input),
                    }
                }));
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
    if !annotations.is_empty() {
        message.insert("annotations".to_string(), Value::Array(annotations));
    }
    message.insert(
        "content".to_string(),
        openai_content_value_from_parts(visible_blocks, !tool_calls.is_empty()),
    );
    Value::Object(message)
}

pub(crate) fn canonical_to_openai_responses_response_with_profile(
    canonical: &CanonicalResponse,
    report_context: &Value,
    compact: bool,
) -> Value {
    crate::protocol::formats::openai_responses::response::to_raw(canonical, report_context, compact)
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
    crate::protocol::formats::claude_messages::response::to_raw(canonical)
}

pub fn canonical_to_gemini_response(
    canonical: &CanonicalResponse,
    report_context: &Value,
) -> Option<Value> {
    crate::protocol::formats::gemini_generate_content::response::to_raw(canonical, report_context)
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
        "tool" => CanonicalRole::Tool,
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
                    extensions: BTreeMap::new(),
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
                        extensions: claude_extensions(block, &["type", "text"]),
                    });
                }
            }
            Some(instructions)
        }
        _ => None,
    }
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
            extensions: claude_extensions(block_object, &["type", "thinking", "text", "signature"]),
        }),
        "redacted_thinking" => Some(CanonicalContentBlock::Thinking {
            text: String::new(),
            signature: None,
            encrypted_content: block_object
                .get("data")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            extensions: claude_extensions(block_object, &["type", "data"]),
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
                extensions: claude_extensions(
                    block_object,
                    &["type", "tool_use_id", "content", "is_error"],
                ),
            })
        }
        _ => Some(CanonicalContentBlock::Unknown {
            raw_type,
            payload: block.clone(),
            extensions: BTreeMap::new(),
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
            extensions: BTreeMap::new(),
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
        }
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let tool_call = tool_call.as_object()?;
            let function = tool_call.get("function").and_then(Value::as_object)?;
            blocks.push(CanonicalContentBlock::ToolUse {
                id: tool_call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
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
    if role == CanonicalRole::Tool {
        let text = openai_content_text(message.get("content"));
        blocks.push(CanonicalContentBlock::ToolResult {
            tool_use_id: message
                .get("tool_call_id")
                .and_then(Value::as_str)
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
            extensions: openai_extensions(message, &["role", "content", "tool_call_id"]),
        });
    }
    Some(blocks)
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
                    blocks.push(CanonicalContentBlock::Thinking {
                        text: String::new(),
                        signature: None,
                        encrypted_content: Some(data.to_string()),
                        extensions: openai_extensions(part_object, &["type", "data"]),
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
                    continue;
                }
                let item_object = item.as_object()?;
                let item_type = item_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                    .trim()
                    .to_ascii_lowercase();
                match item_type.as_str() {
                    "message" => {
                        let role = openai_role_to_canonical(
                            item_object
                                .get("role")
                                .and_then(Value::as_str)
                                .unwrap_or("user"),
                        );
                        if matches!(role, CanonicalRole::System | CanonicalRole::Developer) {
                            let text = openai_content_text(item_object.get("content"));
                            if !text.trim().is_empty() {
                                messages.push(CanonicalMessage {
                                    role,
                                    content: vec![CanonicalContentBlock::Text {
                                        text,
                                        extensions: BTreeMap::new(),
                                    }],
                                    extensions: openai_responses_extensions(
                                        item_object,
                                        &["type", "role", "content"],
                                    ),
                                });
                            }
                            continue;
                        }
                        messages.push(CanonicalMessage {
                            role,
                            content: openai_responses_content_to_blocks(
                                item_object.get("content"),
                            )?,
                            extensions: openai_responses_extensions(
                                item_object,
                                &["type", "role", "content"],
                            ),
                        });
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
                            .unwrap_or_else(|| {
                                let generated =
                                    format!("call_auto_{next_generated_tool_call_index}");
                                next_generated_tool_call_index += 1;
                                generated
                            });
                        messages.push(CanonicalMessage {
                            role: CanonicalRole::Assistant,
                            content: vec![CanonicalContentBlock::ToolUse {
                                id,
                                name: name.to_string(),
                                input: parse_jsonish_value(item_object.get("arguments")),
                                extensions: openai_responses_extensions(
                                    item_object,
                                    &["type", "call_id", "id", "name", "arguments"],
                                ),
                            }],
                            extensions: BTreeMap::new(),
                        });
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
                                extensions: openai_responses_extensions(
                                    item_object,
                                    &[
                                        "type",
                                        "call_id",
                                        "tool_call_id",
                                        "id",
                                        "output",
                                        "is_error",
                                    ],
                                ),
                            }],
                            extensions: BTreeMap::new(),
                        });
                    }
                    _ => messages.push(CanonicalMessage {
                        role: CanonicalRole::Unknown,
                        content: vec![CanonicalContentBlock::Unknown {
                            raw_type: item_type,
                            payload: item.clone(),
                            extensions: BTreeMap::new(),
                        }],
                        extensions: BTreeMap::new(),
                    }),
                }
            }
            Some(messages)
        }
        _ => None,
    }
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

pub(crate) fn openai_responses_output_to_canonical_blocks(
    output: Option<&Value>,
) -> Option<Vec<CanonicalContentBlock>> {
    let Some(output) = output else {
        return Some(Vec::new());
    };
    let output_items = output.as_array()?;
    let mut blocks = Vec::new();
    for (index, item) in output_items.iter().enumerate() {
        let Some(item_object) = item.as_object() else {
            blocks.push(CanonicalContentBlock::Unknown {
                raw_type: String::new(),
                payload: item.clone(),
                extensions: BTreeMap::new(),
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
                blocks.extend(openai_responses_content_to_blocks(
                    item_object.get("content"),
                )?);
            }
            "reasoning" => {
                let mut emitted = false;
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
                        blocks.push(CanonicalContentBlock::Thinking {
                            text: text.to_string(),
                            signature: None,
                            encrypted_content: item_object
                                .get("encrypted_content")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned),
                            extensions,
                        });
                        emitted = true;
                    }
                }
                if !emitted {
                    blocks.push(CanonicalContentBlock::Unknown {
                        raw_type: item_type,
                        payload: item.clone(),
                        extensions: BTreeMap::new(),
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
                blocks.push(CanonicalContentBlock::ToolUse {
                    id,
                    name: name.to_string(),
                    input: parse_jsonish_value(item_object.get("arguments")),
                    extensions: openai_responses_extensions(
                        item_object,
                        &["type", "id", "call_id", "name", "arguments", "status"],
                    ),
                });
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
                blocks.push(CanonicalContentBlock::ToolResult {
                    tool_use_id: id,
                    name: None,
                    output,
                    content_text: raw_output.map(openai_responses_tool_output_text),
                    is_error: item_object
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    extensions: openai_responses_extensions(
                        item_object,
                        &[
                            "type",
                            "id",
                            "call_id",
                            "tool_call_id",
                            "output",
                            "is_error",
                        ],
                    ),
                });
            }
            "output_text" | "text" | "output_image" | "image_url" | "file" | "input_file"
            | "input_audio" => blocks.push(openai_responses_part_to_canonical_block(item)?),
            _ => blocks.push(CanonicalContentBlock::Unknown {
                raw_type: item_type,
                payload: item.clone(),
                extensions: BTreeMap::new(),
            }),
        }
    }
    Some(blocks)
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
            extensions: openai_responses_extensions(part_object, &["type", "text"]),
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
                extensions
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
        "refusal" => Some(CanonicalContentBlock::Unknown {
            raw_type,
            payload: part.clone(),
            extensions: BTreeMap::new(),
        }),
        _ => Some(CanonicalContentBlock::Unknown {
            raw_type,
            payload: part.clone(),
            extensions: BTreeMap::new(),
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

pub(crate) fn canonical_message_to_openai_chat(message: &CanonicalMessage) -> Value {
    let mut output = Map::new();
    output.insert(
        "role".to_string(),
        Value::String(
            match message.role {
                CanonicalRole::Assistant => "assistant",
                CanonicalRole::System => "system",
                CanonicalRole::Developer => "developer",
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
    for block in &message.content {
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
                id, name, input, ..
            } => tool_calls.push(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": canonicalize_tool_arguments(input),
                }
            })),
            CanonicalContentBlock::ToolResult {
                tool_use_id,
                content_text,
                output: result_output,
                ..
            } => {
                output.insert("role".to_string(), Value::String("tool".to_string()));
                output.insert(
                    "tool_call_id".to_string(),
                    Value::String(tool_use_id.clone()),
                );
                output.insert(
                    "content".to_string(),
                    result_output
                        .clone()
                        .unwrap_or_else(|| Value::String(content_text.clone().unwrap_or_default())),
                );
                return Value::Object(output);
            }
            other => {
                if let Some(part) = canonical_content_block_to_openai_part(other) {
                    content_parts.push(part);
                }
            }
        }
    }
    output.insert(
        "content".to_string(),
        if !tool_calls.is_empty() && content_parts.is_empty() {
            if reasoning_parts.is_empty() {
                Value::Array(Vec::new())
            } else {
                Value::Null
            }
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
    Value::Object(output)
}

pub(crate) fn canonical_content_block_to_openai_part(
    block: &CanonicalContentBlock,
) -> Option<Value> {
    match block {
        CanonicalContentBlock::Text { text, .. } => Some(json!({
            "type": "text",
            "text": text,
        })),
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            detail,
            ..
        } => {
            let mut image = Map::new();
            image.insert(
                "url".to_string(),
                Value::String(media_data_or_url(media_type, data, url)),
            );
            if let Some(detail) = detail {
                image.insert("detail".to_string(), Value::String(detail.clone()));
            }
            Some(json!({
                "type": "image_url",
                "image_url": Value::Object(image),
            }))
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
                file.insert(
                    "file_data".to_string(),
                    Value::String(media_data_or_url(media_type, data, file_url)),
                );
            }
            if let Some(value) = filename {
                file.insert("filename".to_string(), Value::String(value.clone()));
            }
            Some(json!({
                "type": "file",
                "file": Value::Object(file),
            }))
        }
        CanonicalContentBlock::Audio { data, format, .. } => Some(json!({
            "type": "input_audio",
            "input_audio": {
                "data": data.clone().unwrap_or_default(),
                "format": format.clone().unwrap_or_else(|| "mp3".to_string()),
            }
        })),
        CanonicalContentBlock::Thinking { text, .. } => Some(json!({
            "type": "text",
            "text": text,
        })),
        CanonicalContentBlock::ToolUse { .. }
        | CanonicalContentBlock::ToolResult { .. }
        | CanonicalContentBlock::Unknown { .. } => None,
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
                file.insert(
                    "file_data".to_string(),
                    Value::String(media_data_or_url(media_type, data, file_url)),
                );
            }
            if let Some(value) = filename {
                file.insert("filename".to_string(), Value::String(value.clone()));
            }
            Some(json!({
                "type": "file",
                "file": Value::Object(file),
            }))
        }
        CanonicalContentBlock::Audio { data, format, .. } => Some(json!({
            "type": "input_audio",
            "input_audio": {
                "data": data.clone().unwrap_or_default(),
                "format": format.clone().unwrap_or_else(|| "mp3".to_string()),
            }
        })),
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
            extensions: claude_extensions(
                tool_object,
                &["type", "name", "description", "input_schema"],
            ),
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
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" | "xhigh" => Some("xhigh"),
        _ => None,
    }
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
    let mut extensions = BTreeMap::new();
    extensions.insert(
        "gemini".to_string(),
        json!({ "thinking_config": Value::Object(thinking_config.clone()) }),
    );
    if let Some(reasoning_effort) =
        budget_tokens.map(map_thinking_budget_to_openai_reasoning_effort)
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
);

pub(crate) fn gemini_tools_to_canonical(value: Option<&Value>) -> Option<GeminiCanonicalTools> {
    let Some(value) = value else {
        return Some((Vec::new(), Vec::new(), None, None));
    };
    let tools = value.as_array()?;
    let mut canonical = Vec::new();
    let mut builtin_tools = Vec::new();
    let mut web_search_options = None;
    for tool in tools {
        let tool_object = tool.as_object()?;
        if tool_object.get("googleSearch").is_some() || tool_object.get("google_search").is_some() {
            web_search_options = Some(json!({}));
            builtin_tools.push(tool.clone());
        }
        if tool_object.get("codeExecution").is_some()
            || tool_object.get("code_execution").is_some()
            || tool_object.get("urlContext").is_some()
            || tool_object.get("url_context").is_some()
        {
            builtin_tools.push(tool.clone());
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
                extensions: gemini_extensions(
                    declaration_object,
                    &["name", "description", "parameters"],
                ),
            });
        }
    }
    Some((
        canonical,
        builtin_tools,
        web_search_options,
        Some(value.clone()),
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
    tools
        .iter()
        .map(|tool| {
            let tool_object = tool.as_object()?;
            let function = tool_object.get("function").and_then(Value::as_object)?;
            Some(CanonicalToolDefinition {
                name: function.get("name").and_then(Value::as_str)?.to_string(),
                description: function
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parameters: function.get("parameters").cloned(),
                extensions: openai_extensions(tool_object, &["type", "function"]),
            })
        })
        .collect()
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
                let function_extensions =
                    openai_responses_extensions(function, &["name", "description", "parameters"]);
                merge_tool_extensions(&mut extensions, function_extensions);
                canonical.push(CanonicalToolDefinition {
                    name: name.to_string(),
                    description: function
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    parameters: function.get("parameters").cloned(),
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
                extensions: openai_responses_extensions(
                    tool_object,
                    &["type", "name", "description", "parameters"],
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
    json!({
        "type": "function",
        "function": Value::Object(function),
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
            if let Some(data) = encrypted_content.as_ref().filter(|value| !value.is_empty()) {
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
            let mut out = Map::new();
            out.insert("type".to_string(), Value::String("tool_use".to_string()));
            out.insert("id".to_string(), Value::String(id.clone()));
            out.insert("name".to_string(), Value::String(name.clone()));
            out.insert("input".to_string(), input.clone());
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
                Value::String(tool_use_id.clone()),
            );
            out.insert(
                "content".to_string(),
                output
                    .clone()
                    .unwrap_or_else(|| Value::String(content_text.clone().unwrap_or_default())),
            );
            out.insert("is_error".to_string(), Value::Bool(*is_error));
            out.extend(namespace_extension_object(extensions, "claude", &out));
            Some(Some(Value::Object(out)))
        }
        CanonicalContentBlock::Unknown { .. } => Some(None),
    }
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
                tool.parameters.clone().unwrap_or_else(|| json!({})),
            );
            out.extend(namespace_extension_object(&tool.extensions, "claude", &out));
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
        output_object.insert("tools".to_string(), raw_tools);
    }
    if let Some(raw_tool_config) = gemini.get("raw_tool_config").cloned() {
        output_object.insert("toolConfig".to_string(), raw_tool_config);
    }
    Some(())
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
    Some(CanonicalResponseFormat {
        format_type: object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .to_string(),
        json_schema: object.get("json_schema").cloned(),
        extensions: openai_extensions(object, &["type", "json_schema"]),
    })
}

pub(crate) fn canonical_response_format_to_openai(value: &CanonicalResponseFormat) -> Value {
    let mut output = Map::new();
    output.insert("type".to_string(), Value::String(value.format_type.clone()));
    if let Some(json_schema) = &value.json_schema {
        output.insert("json_schema".to_string(), json_schema.clone());
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
                .get("cached_creation_tokens")
                .or_else(|| details.get("cache_creation_tokens"))
        })
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(CanonicalUsage {
        input_tokens,
        output_tokens,
        total_tokens: usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens + output_tokens),
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
    Some(CanonicalUsage {
        input_tokens,
        output_tokens,
        total_tokens: input_tokens + output_tokens,
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
            ],
        ),
        ..CanonicalUsage::default()
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
    let output_tokens = visible_output_tokens + reasoning_tokens;
    Some(CanonicalUsage {
        input_tokens,
        output_tokens,
        total_tokens: usage
            .get("totalTokenCount")
            .or_else(|| usage.get("total_token_count"))
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens + output_tokens),
        reasoning_tokens,
        extensions: gemini_extensions(
            usage,
            &[
                "promptTokenCount",
                "prompt_token_count",
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
    let mut output = json!({
        "prompt_tokens": value.input_tokens,
        "completion_tokens": value.output_tokens,
        "total_tokens": if value.total_tokens > 0 {
            value.total_tokens
        } else {
            value.input_tokens + value.output_tokens
        },
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
        output["prompt_tokens_details"]["cached_creation_tokens"] =
            Value::from(value.cache_write_tokens);
    }
    output
}

pub(crate) fn canonical_usage_to_openai_responses_usage(value: &CanonicalUsage) -> Value {
    let mut output = json!({
        "input_tokens": value.input_tokens,
        "output_tokens": value.output_tokens,
        "total_tokens": if value.total_tokens > 0 {
            value.total_tokens
        } else {
            value.input_tokens + value.output_tokens
        },
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
        output["input_tokens_details"]["cached_creation_tokens"] =
            Value::from(value.cache_write_tokens);
    }
    output
}

pub(crate) fn canonical_usage_to_claude(value: &CanonicalUsage) -> Value {
    let mut output = json!({
        "input_tokens": value.input_tokens,
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
    output
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
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            CanonicalStopReason::ContentFiltered
        }
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

pub(crate) fn gemini_openai_extra_body(request: &Map<String, Value>) -> Option<Value> {
    let mut extra_body = Map::new();
    if let Some(generation_config) = request
        .get("generationConfig")
        .or_else(|| request.get("generation_config"))
        .and_then(Value::as_object)
    {
        let mut google = Map::new();
        if let Some(thinking_config) =
            gemini_value_by_case(generation_config, "thinkingConfig", "thinking_config").cloned()
        {
            google.insert("thinking_config".to_string(), thinking_config);
        }
        if let Some(response_modalities) = gemini_value_by_case(
            generation_config,
            "responseModalities",
            "response_modalities",
        )
        .cloned()
        {
            google.insert("response_modalities".to_string(), response_modalities);
        }
        if !google.is_empty() {
            extra_body.insert("google".to_string(), Value::Object(google));
        }

        let generation_config_extra = gemini_generation_config_extra(generation_config);
        if !generation_config_extra.is_empty() {
            extra_body.insert(
                "gemini".to_string(),
                json!({ "generation_config_extra": generation_config_extra }),
            );
        }
    }
    if let Some(safety_settings) = request
        .get("safetySettings")
        .or_else(|| request.get("safety_settings"))
        .cloned()
    {
        let gemini = extra_body
            .entry("gemini".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()?;
        gemini.insert("safety_settings".to_string(), safety_settings);
    }
    if let Some(cached_content) = request
        .get("cachedContent")
        .or_else(|| request.get("cached_content"))
        .cloned()
    {
        let gemini = extra_body
            .entry("gemini".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()?;
        gemini.insert("cached_content".to_string(), cached_content);
    }
    (!extra_body.is_empty()).then_some(Value::Object(extra_body))
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
        CanonicalContentBlock, CanonicalRole,
    };
    use serde_json::{json, Value};

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

        let rebuilt = canonical_to_openai_chat_request(&canonical);
        assert_eq!(rebuilt["model"], "gpt-5");
        assert_eq!(rebuilt["messages"][0]["role"], "system");
        assert_eq!(rebuilt["messages"][1]["role"], "developer");
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
    fn canonical_request_adapter_keeps_existing_simple_chat_shape() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}],
            "stop": ["x", "y"],
            "n": 2
        });
        let canonical = from_openai_chat_to_canonical_request(&request).expect("canonical request");
        let rebuilt = canonical_to_openai_chat_request(&canonical);
        assert_eq!(rebuilt["model"], request["model"]);
        assert_eq!(rebuilt["messages"], request["messages"]);
        assert_eq!(rebuilt["stop"], Value::Array(vec![json!("x"), json!("y")]));
        assert_eq!(rebuilt["n"], 2);
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
                    "json_schema": {"name": "answer", "schema": {"type": "object"}}
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

        let rebuilt = canonical_to_openai_responses_request(&canonical, "gpt-5-upstream", false)
            .expect("openai responses request");
        assert_eq!(rebuilt["model"], "gpt-5-upstream");
        assert_eq!(rebuilt["text"]["format"]["json_schema"]["name"], "answer");
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
                    "type": "future_item",
                    "payload": true
                }
            ],
            "usage": {
                "input_tokens": 3,
                "input_tokens_details": {"cached_tokens": 2},
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
            |block| matches!(block, CanonicalContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1")
        ));
        assert_eq!(canonical_response_unknown_block_count(&canonical), 2);
        assert_eq!(canonical.usage.as_ref().unwrap().cache_read_tokens, 2);
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
        assert_eq!(rebuilt["usage"]["input_tokens_details"]["cached_tokens"], 2);
        assert_eq!(
            rebuilt["usage"]["output_tokens_details"]["reasoning_tokens"],
            1
        );
        assert_eq!(rebuilt["service_tier"], "flex");
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

        let openai_chat = canonical_to_openai_chat_request(&canonical);
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
                "cache_creation_input_tokens": 2
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

        let rebuilt = canonical_to_claude_response(&canonical);
        assert_eq!(rebuilt["content"][0]["signature"], "sig_123");
        assert_eq!(rebuilt["content"][2]["name"], "lookup");
        assert_eq!(rebuilt["stop_reason"], "tool_use");
        assert_eq!(rebuilt["usage"]["cache_read_input_tokens"], 3);
        assert_eq!(rebuilt["usage"]["cache_creation_input_tokens"], 2);
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
        assert_eq!(canonical.usage.as_ref().unwrap().output_tokens, 7);
        assert_eq!(canonical.usage.as_ref().unwrap().reasoning_tokens, 2);

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
        assert_eq!(rebuilt["usageMetadata"]["thoughtsTokenCount"], 2);
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
