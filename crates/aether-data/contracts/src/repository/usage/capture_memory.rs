use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{Map, Value};

use super::types::UsageBodyCaptureState;

/// Shared accounting for the estimated heap retained by diagnostic JSON bodies.
#[doc(hidden)]
#[derive(Debug)]
pub struct UsageCaptureMemoryBudget {
    limit: usize,
    retained: AtomicUsize,
    downgraded_total: AtomicU64,
}

impl UsageCaptureMemoryBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            retained: AtomicUsize::new(0),
            downgraded_total: AtomicU64::new(0),
        }
    }

    fn try_reserve(&self, bytes: usize) -> bool {
        if bytes == 0 {
            return true;
        }
        self.retained
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |retained| {
                retained
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limit)
            })
            .is_ok()
    }

    fn release(&self, bytes: usize) {
        if bytes != 0 {
            self.retained.fetch_sub(bytes, Ordering::AcqRel);
        }
    }

    fn record_downgrade(&self) {
        self.downgraded_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained.load(Ordering::Acquire)
    }

    pub fn downgraded_total(&self) -> u64 {
        self.downgraded_total.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> (usize, usize, u64) {
        (self.limit, self.retained_bytes(), self.downgraded_total())
    }
}

/// Non-serialized ownership of the diagnostic JSON heap estimate.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct UsageCaptureRetention {
    budget: Option<Arc<UsageCaptureMemoryBudget>>,
    bytes: usize,
}

// Runtime accounting does not participate in value or wire equality.
impl PartialEq for UsageCaptureRetention {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl UsageCaptureRetention {
    pub fn reserve(&mut self, budget: Arc<UsageCaptureMemoryBudget>, bytes: usize) -> bool {
        if self
            .budget
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &budget))
        {
            if bytes > self.bytes && !budget.try_reserve(bytes - self.bytes) {
                budget.record_downgrade();
                return false;
            }
            if bytes < self.bytes {
                budget.release(self.bytes - bytes);
            }
            self.bytes = bytes;
            return true;
        }
        if !budget.try_reserve(bytes) {
            budget.record_downgrade();
            return false;
        }
        *self = Self {
            budget: Some(budget),
            bytes,
        };
        true
    }

    pub fn clear(&mut self, budget: Arc<UsageCaptureMemoryBudget>) {
        *self = Self {
            budget: Some(budget),
            bytes: 0,
        };
    }

    pub fn clone_for_bodies(&self, estimate: impl FnOnce() -> usize) -> (Self, bool) {
        let Some(budget) = &self.budget else {
            return (Self::default(), true);
        };
        let mut retention = Self::default();
        if retention.reserve(Arc::clone(budget), estimate()) {
            (retention, true)
        } else {
            retention.clear(Arc::clone(budget));
            (retention, false)
        }
    }
}

impl Drop for UsageCaptureRetention {
    fn drop(&mut self) {
        if let Some(budget) = &self.budget {
            budget.release(self.bytes);
        }
    }
}

// serde_json::Map does not expose its backing allocation capacity. This charges a
// conservative per-entry estimate, not an allocator or process RSS measurement.
#[doc(hidden)]
pub fn usage_json_heap_estimate(value: &Value) -> usize {
    match value {
        Value::String(value) => value.capacity(),
        Value::Array(values) => values.iter().fold(
            values
                .capacity()
                .saturating_mul(std::mem::size_of::<Value>()),
            |bytes, value| bytes.saturating_add(usage_json_heap_estimate(value)),
        ),
        Value::Object(values) => values.iter().fold(
            values.len().saturating_mul(
                4 * (std::mem::size_of::<String>()
                    + std::mem::size_of::<Value>()
                    + std::mem::size_of::<usize>()),
            ),
            |bytes, (key, value)| {
                bytes
                    .saturating_add(key.capacity())
                    .saturating_add(usage_json_heap_estimate(value))
            },
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

/// Marks an omitted diagnostic body using the existing capture metadata shape.
#[doc(hidden)]
pub fn mark_usage_capture_memory_omitted(metadata: &mut Option<Value>, key: &str) {
    let source_bytes = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("body_capture"))
        .and_then(|capture| capture.get(key))
        .and_then(|entry| entry.get("source_bytes"))
        .and_then(Value::as_u64);
    let Some(metadata) = metadata
        .get_or_insert_with(|| Value::Object(Map::with_capacity(1)))
        .as_object_mut()
    else {
        return;
    };
    let Some(body_capture) = metadata
        .entry("body_capture".to_owned())
        .or_insert_with(|| Value::Object(Map::with_capacity(1)))
        .as_object_mut()
    else {
        return;
    };
    let mut entry = Map::with_capacity(3 + usize::from(source_bytes.is_some()));
    entry.insert(
        "state".to_owned(),
        Value::String(UsageBodyCaptureState::Truncated.as_str().to_owned()),
    );
    entry.insert("stored_bytes".to_owned(), Value::from(0));
    if let Some(source_bytes) = source_bytes {
        entry.insert("source_bytes".to_owned(), Value::from(source_bytes));
    }
    entry.insert(
        "reason".to_owned(),
        Value::String("usage_event_memory_budget_exceeded".to_owned()),
    );
    body_capture.insert(key.to_owned(), Value::Object(entry));
}

#[cfg(test)]
mod tests {
    use super::super::types::UpsertUsageRecord;
    use super::*;
    use serde_json::json;

    fn upsert_with_diagnostic_bodies() -> UpsertUsageRecord {
        serde_json::from_str(
            r#"{
            "request_id": "retained-request",
            "provider_name": "openai",
            "model": "test-model",
            "status": "completed",
            "billing_status": "settled",
            "updated_at_unix_secs": 123,
            "input_tokens": 100,
            "output_tokens": 500,
            "total_tokens": 600,
            "cache_creation_input_tokens": 7,
            "cache_creation_ephemeral_5m_input_tokens": 7,
            "cache_creation_ephemeral_1h_input_tokens": 0,
            "cache_read_input_tokens": 0,
            "total_cost_usd": 1.25,
            "actual_total_cost_usd": 0.75,
            "cache_creation_cost_usd": 0.05,
            "cache_read_cost_usd": 0.0,
            "status_code": 200,
            "error_message": "preserved diagnostic classification",
            "request_headers": {"x-request": "original"},
            "provider_request_headers": {"x-provider-request": "original"},
            "response_headers": {"x-response": "original"},
            "client_response_headers": {"x-client-response": "original"},
            "request_body": {"text": "original request body"},
            "provider_request_body": {"text": "original provider request body"},
            "response_body": {"text": "original provider response body"},
            "client_response_body": {"text": "original client response body"},
            "request_body_ref": "usage://retained-request/request",
            "provider_request_body_ref": "usage://retained-request/provider_request",
            "response_body_ref": "usage://retained-request/response",
            "client_response_body_ref": "usage://retained-request/client_response",
            "request_body_state": "inline",
            "provider_request_body_state": "reference",
            "response_body_state": "inline",
            "client_response_body_state": "truncated",
            "request_metadata": {
                "trace_id": "unchanged",
                "body_capture": {
                    "request": {"state": "inline", "source_bytes": 100},
                    "provider_request": {"state": "reference", "source_bytes": 200},
                    "response": {"state": "inline", "source_bytes": 300},
                    "client_response": {"state": "truncated", "source_bytes": 400}
                }
            }
        }"#,
        )
        .expect("valid usage write fixture")
    }

    fn upsert_body_estimate(record: &UpsertUsageRecord) -> usize {
        [
            &record.request_body,
            &record.provider_request_body,
            &record.response_body,
            &record.client_response_body,
        ]
        .into_iter()
        .flatten()
        .map(|body| std::mem::size_of::<Value>() + usage_json_heap_estimate(body))
        .sum()
    }

    #[test]
    fn usage_capture_memory_upsert_serde_skips_retention_and_preserves_value_equality() {
        let mut source = upsert_with_diagnostic_bodies();
        let weight = upsert_body_estimate(&source);
        let budget = Arc::new(UsageCaptureMemoryBudget::new(weight));
        assert!(source
            .capture_retention
            .reserve(Arc::clone(&budget), weight));
        let mut serialized = serde_json::to_value(&source).unwrap();
        assert!(serialized.get("capture_retention").is_none());
        serialized["capture_retention"] = json!({"bytes": usize::MAX});
        let roundtrip: UpsertUsageRecord = serde_json::from_value(serialized).unwrap();
        assert_eq!(source, roundtrip);
        let unmanaged_clone = roundtrip.clone();
        assert_eq!(source, unmanaged_clone);
        assert!(unmanaged_clone.request_body.is_some());
        assert!(unmanaged_clone.provider_request_body.is_some());
        assert!(unmanaged_clone.response_body.is_some());
        assert!(unmanaged_clone.client_response_body.is_some());
        assert_eq!(budget.retained_bytes(), weight);
        assert_eq!(budget.downgraded_total(), 0);
        drop((roundtrip, unmanaged_clone));
        assert_eq!(budget.retained_bytes(), weight);
        drop(source);
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn usage_capture_memory_upsert_clone_reserves_for_all_four_deep_copies() {
        let mut source = upsert_with_diagnostic_bodies();
        let original = serde_json::to_value(&source).unwrap();
        let weight = upsert_body_estimate(&source);
        let budget = Arc::new(UsageCaptureMemoryBudget::new(weight * 2));
        assert!(source
            .capture_retention
            .reserve(Arc::clone(&budget), weight));
        let cloned = source.clone();
        assert_eq!(source, cloned);
        assert_eq!(budget.retained_bytes(), weight * 2);
        assert_eq!(budget.downgraded_total(), 0);
        for (source_body, cloned_body) in [
            (&source.request_body, &cloned.request_body),
            (&source.provider_request_body, &cloned.provider_request_body),
            (&source.response_body, &cloned.response_body),
            (&source.client_response_body, &cloned.client_response_body),
        ] {
            let source_text = source_body.as_ref().unwrap()["text"].as_str().unwrap();
            let cloned_text = cloned_body.as_ref().unwrap()["text"].as_str().unwrap();
            assert_eq!(source_text, cloned_text);
            assert_ne!(source_text.as_ptr(), cloned_text.as_ptr());
        }
        assert_eq!(serde_json::to_value(&source).unwrap(), original);
        drop(source);
        assert_eq!(budget.retained_bytes(), weight);
        assert_eq!(serde_json::to_value(&cloned).unwrap(), original);
        drop(cloned);
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn usage_capture_memory_upsert_clone_over_budget_only_omits_four_bodies() {
        let mut source = upsert_with_diagnostic_bodies();
        let original = serde_json::to_value(&source).unwrap();
        let weight = upsert_body_estimate(&source);
        let budget = Arc::new(UsageCaptureMemoryBudget::new(weight));
        assert!(source
            .capture_retention
            .reserve(Arc::clone(&budget), weight));
        let cloned = source.clone();
        assert_eq!(budget.retained_bytes(), weight);
        assert_eq!(budget.downgraded_total(), 1);
        assert_eq!(serde_json::to_value(&source).unwrap(), original);

        let mut expected = original;
        for (body, state, key, source_bytes) in [
            ("request_body", "request_body_state", "request", 100),
            (
                "provider_request_body",
                "provider_request_body_state",
                "provider_request",
                200,
            ),
            ("response_body", "response_body_state", "response", 300),
            (
                "client_response_body",
                "client_response_body_state",
                "client_response",
                400,
            ),
        ] {
            expected[body] = Value::Null;
            expected[state] = json!("truncated");
            expected["request_metadata"]["body_capture"][key] = json!({
                "state": "truncated",
                "stored_bytes": 0,
                "source_bytes": source_bytes,
                "reason": "usage_event_memory_budget_exceeded"
            });
        }
        assert_eq!(serde_json::to_value(&cloned).unwrap(), expected);
        assert_eq!(cloned.output_tokens, Some(500));
        assert_eq!(cloned.cache_read_input_tokens, Some(0));
        assert_eq!(cloned.cache_creation_ephemeral_1h_input_tokens, Some(0));
        assert_eq!(cloned.total_cost_usd, Some(1.25));
        assert_eq!(cloned.actual_total_cost_usd, Some(0.75));
        drop(cloned);
        assert_eq!(budget.retained_bytes(), weight);
        drop(source);
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn usage_capture_memory_upsert_clone_preserves_explicit_clearing_states() {
        for state in [
            UsageBodyCaptureState::None,
            UsageBodyCaptureState::Disabled,
            UsageBodyCaptureState::Unavailable,
        ] {
            let mut source = upsert_with_diagnostic_bodies();
            source.request_body_state = Some(state);
            source.provider_request_body_state = Some(state);
            source.response_body_state = Some(state);
            source.client_response_body_state = Some(state);
            let original = serde_json::to_value(&source).unwrap();
            let weight = upsert_body_estimate(&source);
            let budget = Arc::new(UsageCaptureMemoryBudget::new(weight));
            assert!(source
                .capture_retention
                .reserve(Arc::clone(&budget), weight));
            let cloned = source.clone();
            let mut expected = original.clone();
            for body in [
                "request_body",
                "provider_request_body",
                "response_body",
                "client_response_body",
            ] {
                expected[body] = Value::Null;
            }
            assert_eq!(serde_json::to_value(&cloned).unwrap(), expected);
            assert_eq!(serde_json::to_value(&source).unwrap(), original);
            assert_eq!(budget.retained_bytes(), weight);
            assert_eq!(budget.downgraded_total(), 1);
            drop(cloned);
            assert_eq!(budget.retained_bytes(), weight);
            drop(source);
            assert_eq!(budget.retained_bytes(), 0);
        }
    }

    #[test]
    fn usage_capture_memory_metadata_preserves_source_bytes_and_unrelated_metadata() {
        let mut metadata = Some(json!({
            "trace_id": "unchanged",
            "body_capture": {
                "request": {"state": "complete", "source_bytes": 42, "stored_bytes": 42, "extra": true},
                "response": {"state": "complete", "source_bytes": 7}
            }
        }));
        mark_usage_capture_memory_omitted(&mut metadata, "request");
        assert_eq!(
            metadata,
            Some(json!({
                "trace_id": "unchanged",
                "body_capture": {
                    "request": {
                        "state": "truncated",
                        "source_bytes": 42,
                        "stored_bytes": 0,
                        "reason": "usage_event_memory_budget_exceeded"
                    },
                    "response": {"state": "complete", "source_bytes": 7}
                }
            }))
        );
    }

    #[test]
    fn usage_capture_memory_metadata_creates_missing_objects_and_replaces_entries() {
        for mut metadata in [
            None,
            Some(json!({})),
            Some(json!({"body_capture": {}})),
            Some(json!({"body_capture": {"request": null}})),
            Some(json!({"body_capture": {"request": "legacy"}})),
            Some(json!({"body_capture": {"request": {"source_bytes": "42"}}})),
            Some(json!({"body_capture": {"request": {"source_bytes": -1}}})),
        ] {
            mark_usage_capture_memory_omitted(&mut metadata, "request");
            assert_eq!(
                metadata,
                Some(json!({"body_capture": {"request": {
                    "state": "truncated",
                    "stored_bytes": 0,
                    "reason": "usage_event_memory_budget_exceeded"
                }}}))
            );
        }
    }

    #[test]
    fn usage_capture_memory_metadata_preserves_existing_non_object_containers() {
        for metadata in [
            Value::Null,
            json!(false),
            json!(7),
            json!("legacy"),
            json!([]),
            json!({"body_capture": null}),
            json!({"body_capture": false}),
            json!({"body_capture": 7}),
            json!({"body_capture": "legacy"}),
            json!({"body_capture": []}),
        ] {
            let mut actual = Some(metadata.clone());
            mark_usage_capture_memory_omitted(&mut actual, "request");
            assert_eq!(actual, Some(metadata));
        }
    }
}
