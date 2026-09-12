use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use aether_data_contracts::DataLayerError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::body_capture::mark_usage_event_capture_truncated;
pub use crate::event_capture_budget::UsageEventCaptureRetention;
use crate::event_capture_budget::{
    json_heap_estimate, shared_capture_memory_budget, EventCaptureMemoryBudget,
};

pub const USAGE_EVENT_VERSION: u8 = 1;

#[path = "event_wire.rs"]
mod wire;
pub(crate) use wire::EncodedUsageEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventType {
    Pending,
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
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
    #[doc(hidden)]
    #[serde(skip)]
    pub capture_retention: UsageEventCaptureRetention,
}

impl UsageEventData {
    fn capture_heap_estimate(&self) -> usize {
        [
            &self.request_body,
            &self.provider_request_body,
            &self.response_body,
            &self.client_response_body,
        ]
        .into_iter()
        .flatten()
        .fold(0usize, |bytes, body| {
            bytes
                .saturating_add(std::mem::size_of::<Value>())
                .saturating_add(json_heap_estimate(body))
        })
    }

    fn captured_fields(&self) -> [bool; 4] {
        [
            self.request_body.is_some(),
            self.provider_request_body.is_some(),
            self.response_body.is_some(),
            self.client_response_body.is_some(),
        ]
    }

    fn mark_capture_omitted(&mut self, captured: [bool; 4]) {
        for (present, key, state) in [
            (captured[0], "request", &mut self.request_body_state),
            (
                captured[1],
                "provider_request",
                &mut self.provider_request_body_state,
            ),
            (captured[2], "response", &mut self.response_body_state),
            (
                captured[3],
                "client_response",
                &mut self.client_response_body_state,
            ),
        ] {
            if present
                && !matches!(
                    *state,
                    Some(
                        UsageBodyCaptureState::None
                            | UsageBodyCaptureState::Disabled
                            | UsageBodyCaptureState::Unavailable
                    )
                )
            {
                *state = Some(UsageBodyCaptureState::Truncated);
                mark_usage_event_capture_truncated(&mut self.request_metadata, key);
            }
        }
    }

    pub(crate) fn apply_capture_memory_budget(
        &mut self,
        budget: std::sync::Arc<EventCaptureMemoryBudget>,
    ) {
        let bytes = self.capture_heap_estimate();
        if self
            .capture_retention
            .reserve(std::sync::Arc::clone(&budget), bytes)
        {
            return;
        }
        let captured = self.captured_fields();
        self.request_body = None;
        self.provider_request_body = None;
        self.response_body = None;
        self.client_response_body = None;
        self.mark_capture_omitted(captured);
        // The previous lease is released only after the owned JSON bodies are gone.
        self.capture_retention.clear(budget);
    }
}

impl Clone for UsageEventData {
    fn clone(&self) -> Self {
        let (capture_retention, retain_bodies) = self
            .capture_retention
            .clone_for_bodies(|| self.capture_heap_estimate());
        // Enumerate every field so additions require an explicit ownership decision.
        let mut cloned = Self {
            user_id: self.user_id.clone(),
            api_key_id: self.api_key_id.clone(),
            username: self.username.clone(),
            api_key_name: self.api_key_name.clone(),
            provider_name: self.provider_name.clone(),
            model: self.model.clone(),
            target_model: self.target_model.clone(),
            model_id: self.model_id.clone(),
            global_model_id: self.global_model_id.clone(),
            provider_id: self.provider_id.clone(),
            provider_endpoint_id: self.provider_endpoint_id.clone(),
            provider_api_key_id: self.provider_api_key_id.clone(),
            request_type: self.request_type.clone(),
            api_format: self.api_format.clone(),
            api_family: self.api_family.clone(),
            endpoint_kind: self.endpoint_kind.clone(),
            endpoint_api_format: self.endpoint_api_format.clone(),
            provider_api_family: self.provider_api_family.clone(),
            provider_endpoint_kind: self.provider_endpoint_kind.clone(),
            has_format_conversion: self.has_format_conversion,
            is_stream: self.is_stream,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: self.total_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            cache_creation_ephemeral_5m_input_tokens: self.cache_creation_ephemeral_5m_input_tokens,
            cache_creation_ephemeral_1h_input_tokens: self.cache_creation_ephemeral_1h_input_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cache_creation_cost_usd: self.cache_creation_cost_usd,
            cache_read_cost_usd: self.cache_read_cost_usd,
            output_price_per_1m: self.output_price_per_1m,
            total_cost_usd: self.total_cost_usd,
            actual_total_cost_usd: self.actual_total_cost_usd,
            status_code: self.status_code,
            error_message: self.error_message.clone(),
            error_category: self.error_category.clone(),
            response_time_ms: self.response_time_ms,
            first_byte_time_ms: self.first_byte_time_ms,
            request_headers: self.request_headers.clone(),
            request_body: retain_bodies.then(|| self.request_body.clone()).flatten(),
            request_body_ref: self.request_body_ref.clone(),
            request_body_state: self.request_body_state,
            provider_request_headers: self.provider_request_headers.clone(),
            provider_request_body: retain_bodies
                .then(|| self.provider_request_body.clone())
                .flatten(),
            provider_request_body_ref: self.provider_request_body_ref.clone(),
            provider_request_body_state: self.provider_request_body_state,
            response_headers: self.response_headers.clone(),
            response_body: retain_bodies.then(|| self.response_body.clone()).flatten(),
            response_body_ref: self.response_body_ref.clone(),
            response_body_state: self.response_body_state,
            client_response_headers: self.client_response_headers.clone(),
            client_response_body: retain_bodies
                .then(|| self.client_response_body.clone())
                .flatten(),
            client_response_body_ref: self.client_response_body_ref.clone(),
            client_response_body_state: self.client_response_body_state,
            candidate_id: self.candidate_id.clone(),
            candidate_index: self.candidate_index,
            key_name: self.key_name.clone(),
            planner_kind: self.planner_kind.clone(),
            route_family: self.route_family.clone(),
            route_kind: self.route_kind.clone(),
            execution_path: self.execution_path.clone(),
            local_execution_runtime_miss_reason: self.local_execution_runtime_miss_reason.clone(),
            request_metadata: self.request_metadata.clone(),
            capture_retention,
        };
        if !retain_bodies {
            cloned.mark_capture_omitted(self.captured_fields());
        }
        cloned
    }
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

#[derive(Serialize)]
struct BorrowedUsageEventEnvelope<'a, T: ?Sized> {
    v: u8,
    #[serde(rename = "type")]
    event_type: UsageEventType,
    request_id: &'a str,
    timestamp_ms: u64,
    data: &'a T,
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
        let payload = BorrowedUsageEventEnvelope {
            v: USAGE_EVENT_VERSION,
            event_type: self.event_type,
            request_id: &self.request_id,
            timestamp_ms: self.timestamp_ms,
            data: &self.data,
        };
        let payload = serde_json::to_string(&payload).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "failed to serialize usage event payload: {err}"
            ))
        })?;
        Ok(BTreeMap::from([("payload".to_string(), payload)]))
    }

    pub(crate) fn to_bounded_stream_fields(
        &self,
        max_bytes: usize,
    ) -> Result<EncodedUsageEvent, DataLayerError> {
        wire::encode(self, max_bytes)
    }

    pub fn from_stream_fields(fields: &BTreeMap<String, String>) -> Result<Self, DataLayerError> {
        Self::from_stream_fields_with_capture_budget(fields, shared_capture_memory_budget())
    }

    pub(crate) fn from_stream_fields_with_capture_budget(
        fields: &BTreeMap<String, String>,
        budget: Arc<EventCaptureMemoryBudget>,
    ) -> Result<Self, DataLayerError> {
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

        let mut event = Self {
            event_type: envelope.event_type,
            request_id: envelope.request_id,
            timestamp_ms: envelope.timestamp_ms,
            data: envelope.data,
        };
        // The wire format has no ownership lease. Preserve billing facts before a decoded
        // body can be omitted; the raw Redis response and serde allocation are not budgeted here.
        crate::runtime::prepare_decoded_event_capture_memory(&mut event, budget);
        Ok(event)
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
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use aether_data_contracts::repository::usage::UsageBodyCaptureState;
    use aether_data_contracts::DataLayerError;
    use serde_json::json;

    use crate::event_capture_budget::EventCaptureMemoryBudget;
    use crate::{
        apply_usage_body_capture_policy_to_event, build_upsert_usage_record_from_event,
        UsageBodyCapturePolicy,
    };

    use super::{UsageEvent, UsageEventData, UsageEventType};

    fn captured_event() -> UsageEvent {
        UsageEvent {
            event_type: UsageEventType::Failed,
            request_id: "capture-budget-request".to_string(),
            timestamp_ms: 123_456,
            data: UsageEventData {
                provider_name: "provider".to_string(),
                model: "model".to_string(),
                input_tokens: Some(100),
                output_tokens: Some(500),
                total_tokens: Some(600),
                cache_read_input_tokens: Some(0),
                cache_creation_input_tokens: Some(25),
                actual_total_cost_usd: Some(1.25),
                status_code: Some(502),
                error_category: Some("upstream_error".to_string()),
                error_message: Some("upstream failed".to_string()),
                request_body: Some(json!({"messages": [{"content": "request"}]})),
                provider_request_body: Some(json!({"input": "upstream request"})),
                response_body: Some(json!({"usage": {"input_tokens": 100, "output_tokens": 500}})),
                client_response_body: Some(json!({"error": "client response"})),
                request_body_state: Some(UsageBodyCaptureState::Inline),
                provider_request_body_state: Some(UsageBodyCaptureState::Inline),
                response_body_state: Some(UsageBodyCaptureState::Inline),
                client_response_body_state: Some(UsageBodyCaptureState::Inline),
                request_metadata: Some(json!({
                    "requested_reasoning_effort": "high",
                    "provider_reasoning_effort": "medium",
                    "provider_service_tier": "priority",
                    "provider_actual_service_tier": "default",
                    "provider_cache_ttl_minutes": 60,
                    "plan_usage_reservation_token": "550e8400-e29b-41d4-a716-446655440000",
                    "body_capture": {"response": {"state": "inline", "source_bytes": 1000}}
                })),
                ..UsageEventData::default()
            },
        }
    }

    #[test]
    fn event_capture_budget_zero_preserves_billing_refs_and_database_truncation() {
        let budget = Arc::new(EventCaptureMemoryBudget::new(0));
        let mut event = captured_event();
        event.data.request_body_ref =
            Some("usage://capture-budget-request/request_body".to_string());
        event.data.response_body_ref =
            Some("usage://capture-budget-request/response_body".to_string());
        event.data.apply_capture_memory_budget(Arc::clone(&budget));
        assert!(event.data.request_body.is_none());
        assert!(event.data.provider_request_body.is_none());
        assert!(event.data.response_body.is_none());
        assert!(event.data.client_response_body.is_none());
        let capture_metadata = event
            .data
            .request_metadata
            .as_ref()
            .expect("capture metadata");
        assert_eq!(
            capture_metadata["body_capture"]["response"]["source_bytes"],
            1000
        );
        assert_eq!(
            capture_metadata["body_capture"]["response"]["stored_bytes"],
            0
        );
        assert_eq!(
            capture_metadata["body_capture"]["response"]["reason"],
            "usage_event_memory_budget_exceeded"
        );
        let record = build_upsert_usage_record_from_event(&event).expect("record mapping");
        assert_eq!(record.status, "failed");
        assert_eq!(record.input_tokens, Some(100));
        assert_eq!(record.output_tokens, Some(500));
        assert_eq!(record.cache_read_input_tokens, Some(0));
        assert_eq!(record.cache_creation_input_tokens, Some(25));
        assert_eq!(record.actual_total_cost_usd, Some(1.25));
        assert_eq!(record.error_category.as_deref(), Some("upstream_error"));
        assert_eq!(record.request_body_ref, event.data.request_body_ref);
        assert_eq!(record.response_body_ref, event.data.response_body_ref);
        for state in [
            record.request_body_state,
            record.provider_request_body_state,
            record.response_body_state,
            record.client_response_body_state,
        ] {
            assert_eq!(state, Some(UsageBodyCaptureState::Truncated));
        }
        let metadata = record.request_metadata.expect("preserved metadata");
        assert_eq!(metadata["provider_service_tier"], "priority");
        assert_eq!(metadata["provider_actual_service_tier"], "default");
        assert_eq!(metadata["provider_cache_ttl_minutes"], 60);
        assert_eq!(
            metadata["plan_usage_reservation_token"],
            "550e8400-e29b-41d4-a716-446655440000"
        );
        // Persistence projects billing metadata; capture state remains in typed columns.
        assert!(metadata.get("body_capture").is_none());
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(budget.downgraded_total(), 1);
    }

    #[test]
    fn event_capture_budget_clone_reserves_each_copy_before_cloning_bodies() {
        let mut event = captured_event();
        let weight = event.data.capture_heap_estimate();
        let budget = Arc::new(EventCaptureMemoryBudget::new(weight * 2));
        event.data.apply_capture_memory_budget(Arc::clone(&budget));
        let copy = event.clone();
        assert_eq!(copy, event);
        assert_eq!(budget.retained_bytes(), weight * 2);
        let downgraded = event.clone();
        assert!(event.data.response_body.is_some());
        assert!(copy.data.response_body.is_some());
        assert!(downgraded.data.response_body.is_none());
        assert_eq!(
            downgraded.data.response_body_state,
            Some(UsageBodyCaptureState::Truncated)
        );
        assert_eq!(downgraded.data.total_tokens, event.data.total_tokens);
        assert_eq!(downgraded.timestamp_ms, event.timestamp_ms);
        assert_eq!(downgraded.event_type, event.event_type);
        assert_eq!(budget.retained_bytes(), weight * 2);
        drop(copy);
        assert_eq!(budget.retained_bytes(), weight);
        drop((event, downgraded));
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn event_capture_budget_serialization_borrows_bodies_without_charging_a_clone() {
        let mut event = captured_event();
        let weight = event.data.capture_heap_estimate();
        let budget = Arc::new(EventCaptureMemoryBudget::new(weight));
        crate::runtime::prepare_event_capture_memory(&mut event, Arc::clone(&budget));
        let decoded_budget = Arc::new(EventCaptureMemoryBudget::new(usize::MAX));
        for _ in 0..3 {
            let fields = event.to_stream_fields().expect("wire serialization");
            assert!(!fields["payload"].contains("capture_retention"));
            let decoded = UsageEvent::from_stream_fields_with_capture_budget(
                &fields,
                Arc::clone(&decoded_budget),
            )
            .expect("wire decode");
            assert_eq!(decoded, event);
            assert_eq!(budget.retained_bytes(), weight);
            assert!(decoded_budget.retained_bytes() > 0);
            drop(decoded);
            assert_eq!(decoded_budget.retained_bytes(), 0);
        }
        assert_eq!(budget.downgraded_total(), 0);
        drop(event);
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn from_stream_fields_legacy_body_budget_preserves_billing_and_request_facts() {
        let mut event = captured_event();
        event.data.model = "gpt-5.6-sol".to_string();
        event.data.endpoint_api_format = Some("openai:responses".to_string());
        event.data.request_body = Some(json!({"reasoning": {"effort": "high"}}));
        event.data.provider_request_body = Some(json!({
            "model": "gpt-5.6-sol", "reasoning": {"effort": "medium"},
            "service_tier": "priority"
        }));
        event.data.response_body = Some(json!({"service_tier": "Default"}));
        event.data.request_body_state = None;
        event.data.provider_request_body_state = None;
        event.data.response_body_state = None;
        event.data.client_response_body_state = None;
        event.data.cache_creation_ephemeral_5m_input_tokens = Some(0);
        event.data.cache_creation_ephemeral_1h_input_tokens = Some(25);
        event.data.cache_read_cost_usd = Some(0.0);
        event.data.request_body_ref = Some("usage://legacy/request".to_string());
        event.data.request_metadata = Some(json!({
            "plan_usage_reservation_token": "550e8400-e29b-41d4-a716-446655440000"
        }));
        let fields = event.to_stream_fields().expect("legacy wire serialization");
        assert!(!fields["payload"].contains("request_body_state"));
        let budget = Arc::new(EventCaptureMemoryBudget::new(0));
        let decoded =
            UsageEvent::from_stream_fields_with_capture_budget(&fields, Arc::clone(&budget))
                .expect("legacy wire decode");

        assert_eq!(decoded.event_type, UsageEventType::Failed);
        assert_eq!(decoded.request_id, event.request_id);
        assert_eq!(decoded.timestamp_ms, event.timestamp_ms);
        assert_eq!(decoded.data.input_tokens, Some(100));
        assert_eq!(decoded.data.output_tokens, Some(500));
        assert_eq!(decoded.data.total_tokens, Some(600));
        assert_eq!(decoded.data.cache_creation_input_tokens, Some(25));
        assert_eq!(
            decoded.data.cache_creation_ephemeral_5m_input_tokens,
            Some(0)
        );
        assert_eq!(
            decoded.data.cache_creation_ephemeral_1h_input_tokens,
            Some(25)
        );
        assert_eq!(decoded.data.cache_read_input_tokens, Some(0));
        assert_eq!(decoded.data.cache_read_cost_usd, Some(0.0));
        assert_eq!(decoded.data.actual_total_cost_usd, Some(1.25));
        assert_eq!(decoded.data.status_code, Some(502));
        assert_eq!(
            decoded.data.error_category.as_deref(),
            Some("upstream_error")
        );
        assert_eq!(decoded.data.request_body_ref, event.data.request_body_ref);
        assert!(decoded.data.request_body.is_none());
        assert!(decoded.data.provider_request_body.is_none());
        assert!(decoded.data.response_body.is_none());
        assert!(decoded.data.client_response_body.is_none());
        for state in [
            decoded.data.request_body_state,
            decoded.data.provider_request_body_state,
            decoded.data.response_body_state,
            decoded.data.client_response_body_state,
        ] {
            assert_eq!(state, Some(UsageBodyCaptureState::Truncated));
        }
        let metadata = decoded
            .data
            .request_metadata
            .as_ref()
            .expect("preserved facts");
        assert_eq!(metadata["requested_reasoning_effort"], "high");
        assert_eq!(metadata["provider_reasoning_effort"], "medium");
        assert_eq!(metadata["provider_service_tier"], "priority");
        assert_eq!(metadata["provider_actual_service_tier"], "default");
        assert_eq!(metadata["provider_cache_ttl_minutes"], 30);
        assert_eq!(
            metadata["plan_usage_reservation_token"],
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(budget.downgraded_total(), 1);
    }

    #[test]
    fn from_stream_fields_reconstructed_lease_also_bounds_recorder_clones() {
        let fields = captured_event()
            .to_stream_fields()
            .expect("wire serialization");
        let probe_budget = Arc::new(EventCaptureMemoryBudget::new(usize::MAX));
        let probe =
            UsageEvent::from_stream_fields_with_capture_budget(&fields, Arc::clone(&probe_budget))
                .expect("estimate decoded allocation");
        let weight = probe_budget.retained_bytes();
        assert!(weight > 0);
        drop(probe);
        assert_eq!(probe_budget.retained_bytes(), 0);

        let budget = Arc::new(EventCaptureMemoryBudget::new(weight * 2));
        let event =
            UsageEvent::from_stream_fields_with_capture_budget(&fields, Arc::clone(&budget))
                .expect("wire decode");
        let recorder_copy = event.clone();
        assert!(recorder_copy.data.response_body.is_some());
        assert_eq!(budget.retained_bytes(), weight * 2);
        let omitted_copy = event.clone();
        assert!(omitted_copy.data.response_body.is_none());
        assert_eq!(omitted_copy.data.total_tokens, Some(600));
        assert_eq!(omitted_copy.data.cache_read_input_tokens, Some(0));
        assert_eq!(budget.retained_bytes(), weight * 2);
        drop(event);
        assert_eq!(budget.retained_bytes(), weight);
        drop((recorder_copy, omitted_copy));
        assert_eq!(budget.retained_bytes(), 0);
    }

    fn assert_typed_body_clear_is_preserved(event: &UsageEvent, state: UsageBodyCaptureState) {
        for (body, reference, actual_state) in [
            (
                &event.data.request_body,
                &event.data.request_body_ref,
                event.data.request_body_state,
            ),
            (
                &event.data.provider_request_body,
                &event.data.provider_request_body_ref,
                event.data.provider_request_body_state,
            ),
            (
                &event.data.response_body,
                &event.data.response_body_ref,
                event.data.response_body_state,
            ),
            (
                &event.data.client_response_body,
                &event.data.client_response_body_ref,
                event.data.client_response_body_state,
            ),
        ] {
            assert!(body.is_none());
            assert_eq!(actual_state, Some(state));
            assert_eq!(reference.as_deref(), Some("usage://stale/reference"));
        }
        assert!(event
            .data
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("body_capture"))
            .is_none());
    }

    #[test]
    fn from_stream_fields_and_clone_budget_preserve_typed_clear_with_residual_bodies() {
        for state in [
            UsageBodyCaptureState::None,
            UsageBodyCaptureState::Disabled,
            UsageBodyCaptureState::Unavailable,
        ] {
            let mut source = captured_event();
            source.data.request_metadata = None;
            source.data.request_body_state = Some(state);
            source.data.provider_request_body_state = Some(state);
            source.data.response_body_state = Some(state);
            source.data.client_response_body_state = Some(state);
            source.data.request_body_ref = Some("usage://stale/reference".to_string());
            source.data.provider_request_body_ref = Some("usage://stale/reference".to_string());
            source.data.response_body_ref = Some("usage://stale/reference".to_string());
            source.data.client_response_body_ref = Some("usage://stale/reference".to_string());
            let fields = source.to_stream_fields().expect("wire serialization");
            let decoded_budget = Arc::new(EventCaptureMemoryBudget::new(0));
            let decoded = UsageEvent::from_stream_fields_with_capture_budget(
                &fields,
                Arc::clone(&decoded_budget),
            )
            .expect("wire decode");
            assert_typed_body_clear_is_preserved(&decoded, state);
            assert_eq!(decoded_budget.retained_bytes(), 0);

            let clone_budget = Arc::new(EventCaptureMemoryBudget::new(
                source.data.capture_heap_estimate(),
            ));
            crate::runtime::prepare_event_capture_memory(&mut source, Arc::clone(&clone_budget));
            let cloned = source.clone();
            assert_typed_body_clear_is_preserved(&cloned, state);
            assert!(source.data.request_body.is_some());
            assert_eq!(clone_budget.downgraded_total(), 1);
            drop(source);
            assert_eq!(clone_budget.retained_bytes(), 0);
        }
    }

    #[test]
    fn from_stream_fields_legacy_metadata_only_facts_survive_before_billing() {
        for limit in [0, 8192] {
            for include_response in [false, true] {
                let mut source = legacy_metadata_only_event();
                if include_response {
                    source.data.response_body = Some(json!({"result": "response capture"}));
                }
                let fields = source
                    .to_stream_fields()
                    .expect("legacy wire serialization");
                let budget = Arc::new(EventCaptureMemoryBudget::new(limit));
                let decoded = UsageEvent::from_stream_fields_with_capture_budget(
                    &fields,
                    Arc::clone(&budget),
                )
                .expect("legacy wire decode");
                // The worker enriches this clone before DTO conversion. Missing legacy bodies
                // must not erase a previously derived TTL or turn an explicit zero into unknown.
                let billing_event = decoded.clone();
                let metadata = billing_event
                    .data
                    .request_metadata
                    .as_ref()
                    .expect("legacy facts");
                assert_eq!(metadata["requested_reasoning_effort"], "high");
                assert_eq!(metadata["provider_reasoning_effort"], "medium");
                assert_eq!(metadata["provider_service_tier"], "priority");
                assert_eq!(metadata["provider_actual_service_tier"], "default");
                assert_eq!(metadata["provider_cache_ttl_minutes"], 60);
                assert_eq!(billing_event.data.input_tokens, Some(0));
                assert_eq!(billing_event.data.output_tokens, Some(0));
                assert_eq!(billing_event.data.total_tokens, Some(0));
                assert_eq!(billing_event.data.cache_read_input_tokens, Some(0));
                assert_eq!(billing_event.data.cache_creation_input_tokens, Some(0));
                assert_eq!(billing_event.data.actual_total_cost_usd, Some(0.0));
                assert_eq!(billing_event.data.request_body_state, None);
                assert_eq!(billing_event.data.provider_request_body_state, None);
                drop((billing_event, decoded));
                assert_eq!(budget.retained_bytes(), 0);
            }
        }
    }

    #[test]
    fn from_stream_fields_typed_none_still_clears_metadata_only_request_facts() {
        for limit in [0, 8192] {
            let mut source = legacy_metadata_only_event();
            source.data.request_body_state = Some(UsageBodyCaptureState::None);
            source.data.provider_request_body_state = Some(UsageBodyCaptureState::None);
            let fields = source.to_stream_fields().expect("wire serialization");
            let budget = Arc::new(EventCaptureMemoryBudget::new(limit));
            let decoded =
                UsageEvent::from_stream_fields_with_capture_budget(&fields, Arc::clone(&budget))
                    .expect("wire decode");
            let metadata = decoded
                .data
                .request_metadata
                .as_ref()
                .expect("response facts remain");
            for key in [
                "requested_reasoning_effort",
                "provider_reasoning_effort",
                "provider_service_tier",
                "provider_cache_ttl_minutes",
            ] {
                assert!(metadata.get(key).is_none(), "typed none must clear {key}");
            }
            assert_eq!(metadata["provider_actual_service_tier"], "default");
            assert_eq!(
                decoded.data.request_body_state,
                Some(UsageBodyCaptureState::None)
            );
            assert_eq!(
                decoded.data.provider_request_body_state,
                Some(UsageBodyCaptureState::None)
            );
            assert_eq!(decoded.data.cache_read_input_tokens, Some(0));
            assert_eq!(budget.retained_bytes(), 0);
            assert_eq!(budget.downgraded_total(), 0);
        }
    }

    fn legacy_metadata_only_event() -> UsageEvent {
        UsageEvent::new(
            UsageEventType::Completed,
            "legacy-metadata-only",
            UsageEventData {
                provider_name: "openai".to_string(),
                model: "gpt-5.6-sol".to_string(),
                endpoint_api_format: Some("openai:responses".to_string()),
                input_tokens: Some(0),
                output_tokens: Some(0),
                total_tokens: Some(0),
                cache_read_input_tokens: Some(0),
                cache_creation_input_tokens: Some(0),
                actual_total_cost_usd: Some(0.0),
                request_metadata: Some(json!({
                    "requested_reasoning_effort": "high",
                    "provider_reasoning_effort": "medium",
                    "provider_service_tier": "priority",
                    "provider_actual_service_tier": "default",
                    "provider_cache_ttl_minutes": 60
                })),
                ..UsageEventData::default()
            },
        )
    }

    #[test]
    fn from_stream_fields_body_omission_keeps_unknown_usage_unknown() {
        let budget = Arc::new(EventCaptureMemoryBudget::new(0));
        for event_type in [
            UsageEventType::Completed,
            UsageEventType::Failed,
            UsageEventType::Cancelled,
        ] {
            let event = UsageEvent::new(
                event_type,
                "usage-unavailable",
                UsageEventData {
                    provider_name: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    response_body: Some(json!({"error": "usage unavailable"})),
                    request_metadata: Some(json!({
                        "usage_available": false,
                        "usage_pricing_available": false
                    })),
                    ..UsageEventData::default()
                },
            );
            let fields = event.to_stream_fields().expect("wire serialization");
            let decoded =
                UsageEvent::from_stream_fields_with_capture_budget(&fields, Arc::clone(&budget))
                    .expect("wire decode");
            assert_eq!(decoded.event_type, event_type);
            assert_eq!(decoded.data.input_tokens, None);
            assert_eq!(decoded.data.output_tokens, None);
            assert_eq!(decoded.data.total_tokens, None);
            assert_eq!(decoded.data.cache_read_input_tokens, None);
            assert_eq!(decoded.data.cache_creation_input_tokens, None);
            assert_eq!(decoded.data.actual_total_cost_usd, None);
            assert_eq!(
                decoded.data.request_metadata.as_ref().expect("metadata")["usage_available"],
                false
            );
            assert_eq!(
                decoded.data.request_metadata.as_ref().expect("metadata")
                    ["usage_pricing_available"],
                false
            );
            assert_eq!(
                decoded.data.response_body_state,
                Some(UsageBodyCaptureState::Truncated)
            );
        }
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(budget.downgraded_total(), 3);
    }

    #[test]
    fn from_stream_fields_invalid_envelopes_do_not_reserve_capture_memory() {
        let budget = Arc::new(EventCaptureMemoryBudget::new(1024));
        let mut unsupported = captured_event()
            .to_stream_fields()
            .expect("wire serialization");
        let mut payload: serde_json::Value =
            serde_json::from_str(&unsupported["payload"]).expect("json");
        payload["v"] = json!(99);
        unsupported.insert("payload".to_string(), payload.to_string());
        for fields in [
            BTreeMap::new(),
            BTreeMap::from([("payload".to_string(), "not json".to_string())]),
            unsupported,
        ] {
            assert!(matches!(
                UsageEvent::from_stream_fields_with_capture_budget(&fields, Arc::clone(&budget)),
                Err(DataLayerError::UnexpectedValue(_))
            ));
            assert_eq!(budget.retained_bytes(), 0);
            assert_eq!(budget.downgraded_total(), 0);
        }
    }

    #[test]
    fn event_capture_budget_basic_policy_needs_no_diagnostic_allocation() {
        let mut event = captured_event();
        let budget = Arc::new(EventCaptureMemoryBudget::new(0));
        apply_usage_body_capture_policy_to_event(UsageBodyCapturePolicy::default(), &mut event);
        event.data.apply_capture_memory_budget(Arc::clone(&budget));
        assert_eq!(
            event.data.response_body_state,
            Some(UsageBodyCaptureState::Disabled)
        );
        assert_eq!(event.data.total_tokens, Some(600));
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(budget.downgraded_total(), 0);
    }

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
