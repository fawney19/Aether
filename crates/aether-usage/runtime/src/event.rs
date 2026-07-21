use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use aether_data_contracts::DataLayerError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const USAGE_EVENT_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventType {
    Pending,
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageEventData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_name: Option<String>,
    pub provider_name: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_api_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_api_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_api_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_format_conversion: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_ephemeral_5m_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_ephemeral_1h_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price_per_1m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_total_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_byte_time_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_body_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_execution_runtime_miss_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub event_type: UsageEventType,
    pub request_id: String,
    pub timestamp_ms: u64,
    pub data: UsageEventData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct UsageEventEnvelope {
    v: u8,
    #[serde(rename = "type")]
    event_type: UsageEventType,
    request_id: String,
    timestamp_ms: u64,
    data: UsageEventData,
}

impl UsageEvent {
    pub fn new(
        event_type: UsageEventType,
        request_id: impl Into<String>,
        data: UsageEventData,
    ) -> Self {
        Self {
            event_type,
            request_id: request_id.into(),
            timestamp_ms: now_ms(),
            data,
        }
    }

    pub fn to_stream_fields(&self) -> Result<BTreeMap<String, String>, DataLayerError> {
        let payload = UsageEventEnvelope {
            v: USAGE_EVENT_VERSION,
            event_type: self.event_type,
            request_id: self.request_id.clone(),
            timestamp_ms: self.timestamp_ms,
            data: self.data.clone(),
        };
        let payload = serde_json::to_string(&payload).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "failed to serialize usage event payload: {err}"
            ))
        })?;
        Ok(BTreeMap::from([("payload".to_string(), payload)]))
    }

    pub fn from_stream_fields(fields: &BTreeMap<String, String>) -> Result<Self, DataLayerError> {
        let payload = fields.get("payload").ok_or_else(|| {
            DataLayerError::UnexpectedValue(
                "usage event stream entry missing payload field".to_string(),
            )
        })?;
        let envelope: UsageEventEnvelope = serde_json::from_str(payload).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "failed to deserialize usage event payload: {err}"
            ))
        })?;
        if envelope.v != USAGE_EVENT_VERSION {
            return Err(DataLayerError::UnexpectedValue(format!(
                "unsupported usage event version: {}",
                envelope.v
            )));
        }

        Ok(Self {
            event_type: envelope.event_type,
            request_id: envelope.request_id,
            timestamp_ms: envelope.timestamp_ms,
            data: envelope.data,
        })
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{UsageEvent, UsageEventData, UsageEventType};

    #[test]
    fn usage_event_round_trips_through_stream_fields() {
        let event = UsageEvent::new(
            UsageEventType::Completed,
            "req-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                input_tokens: Some(10),
                output_tokens: Some(20),
                ..UsageEventData::default()
            },
        );

        let fields = event.to_stream_fields().expect("event should serialize");
        let parsed = UsageEvent::from_stream_fields(&fields).expect("event should parse");

        assert_eq!(parsed.request_id, "req-1");
        assert_eq!(parsed.event_type, UsageEventType::Completed);
        assert_eq!(parsed.data.total_tokens, None);
        assert_eq!(parsed.data.output_tokens, Some(20));
    }
}
