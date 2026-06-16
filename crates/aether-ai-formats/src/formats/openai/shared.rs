use serde_json::{Map, Value};

use crate::formats::shared::model_directives::ReasoningEffort;

macro_rules! define_openai_reasoning_effort {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            None,
            Minimal,
            Low,
            Medium,
            High,
            XHigh,
        }

        impl $name {
            pub fn parse(value: &str) -> Option<Self> {
                match value.trim().to_ascii_lowercase().as_str() {
                    "none" => Some(Self::None),
                    "minimal" => Some(Self::Minimal),
                    "low" => Some(Self::Low),
                    "medium" => Some(Self::Medium),
                    "high" => Some(Self::High),
                    "xhigh" => Some(Self::XHigh),
                    _ => None,
                }
            }

            pub fn as_str(self) -> &'static str {
                match self {
                    Self::None => "none",
                    Self::Minimal => "minimal",
                    Self::Low => "low",
                    Self::Medium => "medium",
                    Self::High => "high",
                    Self::XHigh => "xhigh",
                }
            }
        }
    };
}

define_openai_reasoning_effort!(OpenAiChatReasoningEffort);
define_openai_reasoning_effort!(OpenAiResponsesReasoningEffort);

#[deprecated(note = "use OpenAiChatReasoningEffort or OpenAiResponsesReasoningEffort")]
pub type OpenAiReasoningEffort = OpenAiChatReasoningEffort;

pub fn parse_openai_stop_sequences(stop: Option<&Value>) -> Option<Vec<Value>> {
    match stop {
        Some(Value::String(value)) if !value.trim().is_empty() => {
            Some(vec![Value::String(value.clone())])
        }
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .collect::<Vec<_>>(),
        )
        .filter(|values| !values.is_empty()),
        _ => None,
    }
}

pub fn resolve_openai_chat_max_tokens(request: &Map<String, Value>) -> u64 {
    request
        .get("max_completion_tokens")
        .and_then(value_as_u64)
        .or_else(|| request.get("max_tokens").and_then(value_as_u64))
        .unwrap_or(4096)
}

pub fn value_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
}

pub fn copy_request_number_field(
    request: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    copy_request_number_field_as(request, target, key, key);
}

pub fn copy_request_number_field_as(
    request: &Map<String, Value>,
    target: &mut Map<String, Value>,
    source_key: &str,
    target_key: &str,
) {
    if let Some(value) = request.get(source_key).cloned() {
        if value.is_number() {
            target.insert(target_key.to_string(), value);
        }
    }
}

pub fn map_openai_reasoning_effort_to_claude_output(value: &str) -> Option<&'static str> {
    ReasoningEffort::parse(value).map(ReasoningEffort::as_claude_output_value)
}

pub fn map_openai_reasoning_effort_to_thinking_budget(value: &str) -> Option<u64> {
    ReasoningEffort::parse(value).map(ReasoningEffort::thinking_budget_tokens)
}

pub fn map_openai_reasoning_effort_to_gemini_budget(value: &str) -> Option<u64> {
    map_openai_reasoning_effort_to_thinking_budget(value)
}

pub fn map_thinking_budget_to_openai_reasoning_effort(value: u64) -> &'static str {
    match value {
        0..=1664 => "low",
        1665..=3072 => "medium",
        3073..=6144 => "high",
        _ => "xhigh",
    }
}

pub fn extract_openai_reasoning_effort(request: &Map<String, Value>) -> Option<String> {
    request
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| {
            request
                .get("reasoning")
                .and_then(Value::as_object)
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}
