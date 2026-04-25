use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StandardizedUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_creation_ephemeral_5m_tokens: i64,
    pub cache_creation_ephemeral_1h_tokens: i64,
    pub cache_read_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_storage_token_hours: f64,
    pub request_count: i64,
    pub dimensions: BTreeMap<String, serde_json::Value>,
}

impl StandardizedUsage {
    pub fn new() -> Self {
        Self {
            request_count: 1,
            ..Self::default()
        }
    }

    pub fn get(&self, field_name: &str) -> Option<serde_json::Value> {
        match field_name {
            "input_tokens" => Some(serde_json::json!(self.input_tokens)),
            "output_tokens" => Some(serde_json::json!(self.output_tokens)),
            "cache_creation_tokens" => Some(serde_json::json!(self.cache_creation_tokens)),
            "cache_creation_ephemeral_5m_tokens" => {
                Some(serde_json::json!(self.cache_creation_ephemeral_5m_tokens))
            }
            "cache_creation_ephemeral_1h_tokens" => {
                Some(serde_json::json!(self.cache_creation_ephemeral_1h_tokens))
            }
            "cache_read_tokens" => Some(serde_json::json!(self.cache_read_tokens)),
            "reasoning_tokens" => Some(serde_json::json!(self.reasoning_tokens)),
            "cache_storage_token_hours" => Some(serde_json::json!(self.cache_storage_token_hours)),
            "request_count" => Some(serde_json::json!(self.request_count)),
            "extra" | "dimensions" => Some(serde_json::json!(self.dimensions)),
            _ => self.dimensions.get(field_name).cloned(),
        }
    }

    pub fn set(&mut self, field_name: &str, value: impl Into<serde_json::Value>) {
        let value = value.into();
        match field_name {
            "input_tokens" => self.input_tokens = as_i64(&value, 0),
            "output_tokens" => self.output_tokens = as_i64(&value, 0),
            "cache_creation_tokens" => self.cache_creation_tokens = as_i64(&value, 0),
            "cache_creation_ephemeral_5m_tokens" => {
                self.cache_creation_ephemeral_5m_tokens = as_i64(&value, 0)
            }
            "cache_creation_ephemeral_1h_tokens" => {
                self.cache_creation_ephemeral_1h_tokens = as_i64(&value, 0)
            }
            "cache_read_tokens" => self.cache_read_tokens = as_i64(&value, 0),
            "reasoning_tokens" => self.reasoning_tokens = as_i64(&value, 0),
            "cache_storage_token_hours" => self.cache_storage_token_hours = as_f64(&value, 0.0),
            "request_count" => self.request_count = as_i64(&value, 0),
            "extra" | "dimensions" => {
                self.dimensions = match value {
                    serde_json::Value::Object(map) => map.into_iter().collect(),
                    _ => BTreeMap::new(),
                }
            }
            _ => {
                self.dimensions.insert(field_name.to_string(), value);
            }
        }
    }

    pub fn normalize_cache_creation_breakdown(mut self) -> Self {
        if self.cache_creation_tokens <= 0 {
            let derived = self
                .cache_creation_ephemeral_5m_tokens
                .saturating_add(self.cache_creation_ephemeral_1h_tokens);
            if derived > 0 {
                self.cache_creation_tokens = derived;
            }
        }
        self
    }

    pub fn signal_score(&self) -> usize {
        [
            self.input_tokens,
            self.output_tokens,
            self.cache_creation_tokens,
            self.cache_creation_ephemeral_5m_tokens,
            self.cache_creation_ephemeral_1h_tokens,
            self.cache_read_tokens,
            self.reasoning_tokens,
        ]
        .into_iter()
        .filter(|value| *value > 0)
        .count()
            + self.dimensions.len()
    }

    pub fn has_token_signal(&self) -> bool {
        self.signal_score() > 0
    }

    pub fn is_more_complete_than(&self, other: &Self) -> bool {
        self.signal_score() > other.signal_score()
    }

    pub fn choose_more_complete(primary: Option<Self>, candidate: Option<Self>) -> Option<Self> {
        match (primary, candidate) {
            (Some(primary), Some(candidate)) if candidate.is_more_complete_than(&primary) => {
                Some(candidate)
            }
            (Some(primary), _) => Some(primary),
            (None, Some(candidate)) => Some(candidate),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExecutionStreamTerminalSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standardized_usage: Option<StandardizedUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub observed_finish: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser_error: Option<String>,
}

fn as_i64(value: &serde_json::Value, default: i64) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
        .unwrap_or(default)
}

fn as_f64(value: &serde_json::Value, default: f64) -> f64 {
    value.as_f64().unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::StandardizedUsage;

    #[test]
    fn standardized_usage_prefers_more_complete_candidate() {
        let mut output_only = StandardizedUsage::new();
        output_only.output_tokens = 131;
        let mut complete = StandardizedUsage::new();
        complete.input_tokens = 26;
        complete.output_tokens = 131;

        let selected = StandardizedUsage::choose_more_complete(Some(output_only), Some(complete))
            .expect("usage should be selected");

        assert_eq!(selected.input_tokens, 26);
        assert_eq!(selected.output_tokens, 131);
    }

    #[test]
    fn standardized_usage_keeps_primary_when_candidate_is_not_more_complete() {
        let mut primary = StandardizedUsage::new();
        primary.input_tokens = 26;
        primary.output_tokens = 131;
        let mut output_only = StandardizedUsage::new();
        output_only.output_tokens = 131;

        let selected = StandardizedUsage::choose_more_complete(Some(primary), Some(output_only))
            .expect("usage should be selected");

        assert_eq!(selected.input_tokens, 26);
        assert_eq!(selected.output_tokens, 131);
    }

    #[test]
    fn standardized_usage_reads_and_writes_known_and_extra_fields() {
        let mut usage = StandardizedUsage::new();
        usage.set("input_tokens", 10);
        usage.set("custom_dimension", "value");

        assert_eq!(usage.get("input_tokens"), Some(serde_json::json!(10)));
        assert_eq!(
            usage.get("custom_dimension"),
            Some(serde_json::json!("value"))
        );
    }
}
