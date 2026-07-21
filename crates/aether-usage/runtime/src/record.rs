use aether_data_contracts::repository::usage::UpsertUsageRecord;
use aether_data_contracts::DataLayerError;

use crate::request_metadata::{
    attach_client_request_body_metadata, attach_provider_request_body_metadata,
    clear_client_request_body_metadata, clear_provider_request_body_metadata,
    request_body_derived_facts_action, sanitize_usage_request_metadata,
    RequestBodyDerivedFactsAction,
};
use crate::{UsageEvent, UsageEventType};

fn metadata_string(metadata: Option<&serde_json::Value>, key: &str) -> Option<String> {
    metadata
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn metadata_u64(metadata: Option<&serde_json::Value>, key: &str) -> Option<u64> {
    metadata
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        })
}

pub fn build_upsert_usage_record_from_event(
    event: &UsageEvent,
) -> Result<UpsertUsageRecord, DataLayerError> {
    let (status, billing_status) = lifecycle_status_and_billing(event.event_type);
    let finalized_at_unix_secs = match event.event_type {
        UsageEventType::Pending | UsageEventType::Streaming => None,
        UsageEventType::Completed | UsageEventType::Failed | UsageEventType::Cancelled => {
            Some(event.timestamp_ms / 1_000)
        }
    };
    let mut data = event.data.clone();
    // Request-derived facts are captured before body capture policy is applied. Do not let a
    // truncation/disabled placeholder clear those facts while converting the queued event into a
    // database record. Inline (or ref-loaded) bodies remain authoritative and may clear stale
    // metadata when the final upstream request no longer contains a value.
    match request_body_derived_facts_action(data.request_body.as_ref(), data.request_body_state) {
        RequestBodyDerivedFactsAction::Refresh => {
            data.request_metadata = attach_client_request_body_metadata(
                data.request_metadata,
                data.request_body.as_ref(),
            );
        }
        RequestBodyDerivedFactsAction::Clear => {
            data.request_metadata = clear_client_request_body_metadata(data.request_metadata);
        }
        RequestBodyDerivedFactsAction::Preserve => {}
    }
    match request_body_derived_facts_action(
        data.provider_request_body.as_ref(),
        data.provider_request_body_state,
    ) {
        RequestBodyDerivedFactsAction::Refresh => {
            data.request_metadata = attach_provider_request_body_metadata(
                data.request_metadata,
                data.endpoint_api_format
                    .as_deref()
                    .or(data.api_format.as_deref()),
                data.target_model.as_deref().or(Some(data.model.as_str())),
                Some(data.model.as_str()),
                data.provider_request_body.as_ref(),
            );
        }
        RequestBodyDerivedFactsAction::Clear => {
            data.request_metadata = clear_provider_request_body_metadata(data.request_metadata);
        }
        RequestBodyDerivedFactsAction::Preserve => {}
    }
    let now_unix_secs = event.timestamp_ms / 1_000;

    Ok(UpsertUsageRecord {
        request_id: event.request_id.clone(),
        user_id: data.user_id,
        api_key_id: data.api_key_id,
        username: data.username,
        api_key_name: data.api_key_name,
        provider_name: data.provider_name,
        model: data.model,
        target_model: data.target_model,
        provider_id: empty_to_none(data.provider_id),
        provider_endpoint_id: empty_to_none(data.provider_endpoint_id),
        provider_api_key_id: empty_to_none(data.provider_api_key_id),
        request_type: data.request_type,
        api_format: data.api_format,
        api_family: data.api_family,
        endpoint_kind: data.endpoint_kind,
        endpoint_api_format: data.endpoint_api_format,
        provider_api_family: data.provider_api_family,
        provider_endpoint_kind: data.provider_endpoint_kind,
        has_format_conversion: data.has_format_conversion,
        is_stream: data.is_stream,
        input_tokens: data.input_tokens,
        output_tokens: data.output_tokens,
        total_tokens: data.total_tokens,
        cache_creation_input_tokens: data.cache_creation_input_tokens,
        cache_creation_ephemeral_5m_input_tokens: data.cache_creation_ephemeral_5m_input_tokens,
        cache_creation_ephemeral_1h_input_tokens: data.cache_creation_ephemeral_1h_input_tokens,
        cache_read_input_tokens: data.cache_read_input_tokens,
        cache_creation_cost_usd: data.cache_creation_cost_usd,
        cache_read_cost_usd: data.cache_read_cost_usd,
        output_price_per_1m: data.output_price_per_1m,
        total_cost_usd: data.total_cost_usd,
        actual_total_cost_usd: data.actual_total_cost_usd,
        status_code: data.status_code,
        error_message: data.error_message,
        error_category: data.error_category,
        response_time_ms: data.response_time_ms,
        first_byte_time_ms: data.first_byte_time_ms,
        status: status.to_string(),
        billing_status: billing_status.to_string(),
        request_headers: data.request_headers,
        request_body: data.request_body,
        request_body_ref: empty_to_none(data.request_body_ref)
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "request_body_ref")),
        request_body_state: data.request_body_state,
        provider_request_headers: data.provider_request_headers,
        provider_request_body: data.provider_request_body,
        provider_request_body_ref: empty_to_none(data.provider_request_body_ref).or_else(|| {
            metadata_string(data.request_metadata.as_ref(), "provider_request_body_ref")
        }),
        provider_request_body_state: data.provider_request_body_state,
        response_headers: data.response_headers,
        response_body: data.response_body,
        response_body_ref: empty_to_none(data.response_body_ref)
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "response_body_ref")),
        response_body_state: data.response_body_state,
        client_response_headers: data.client_response_headers,
        client_response_body: data.client_response_body,
        client_response_body_ref: empty_to_none(data.client_response_body_ref).or_else(|| {
            metadata_string(data.request_metadata.as_ref(), "client_response_body_ref")
        }),
        client_response_body_state: data.client_response_body_state,
        candidate_id: data
            .candidate_id
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "candidate_id")),
        candidate_index: data
            .candidate_index
            .or_else(|| metadata_u64(data.request_metadata.as_ref(), "candidate_index")),
        key_name: data
            .key_name
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "key_name")),
        planner_kind: data
            .planner_kind
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "planner_kind")),
        route_family: data
            .route_family
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "route_family")),
        route_kind: data
            .route_kind
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "route_kind")),
        execution_path: data
            .execution_path
            .or_else(|| metadata_string(data.request_metadata.as_ref(), "execution_path")),
        local_execution_runtime_miss_reason: data.local_execution_runtime_miss_reason.or_else(
            || {
                metadata_string(
                    data.request_metadata.as_ref(),
                    "local_execution_runtime_miss_reason",
                )
            },
        ),
        request_metadata: sanitize_usage_request_metadata(data.request_metadata),
        finalized_at_unix_secs,
        created_at_unix_ms: Some(now_unix_secs),
        updated_at_unix_secs: now_unix_secs,
    })
}

fn lifecycle_status_and_billing(event_type: UsageEventType) -> (&'static str, &'static str) {
    match event_type {
        UsageEventType::Pending => ("pending", "pending"),
        UsageEventType::Streaming => ("streaming", "pending"),
        UsageEventType::Completed => ("completed", "pending"),
        UsageEventType::Failed => ("failed", "void"),
        UsageEventType::Cancelled => ("cancelled", "void"),
    }
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::usage::UsageBodyCaptureState;

    use crate::{UsageEvent, UsageEventData, UsageEventType};

    use super::build_upsert_usage_record_from_event;

    #[test]
    fn builds_upsert_record_from_terminal_event() {
        let record = build_upsert_usage_record_from_event(&UsageEvent {
            event_type: UsageEventType::Completed,
            request_id: "req-1".to_string(),
            timestamp_ms: 1_700_000_000_000,
            data: UsageEventData {
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                api_format: Some("openai:chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
                status_code: Some(200),
                request_body: Some(serde_json::json!({
                    "reasoning": { "effort": "xhigh" }
                })),
                provider_request_body: Some(serde_json::json!({
                    "reasoning": { "effort": "max" },
                    "service_tier": "priority"
                })),
                request_metadata: Some(serde_json::json!({
                    "provider_actual_service_tier": "default"
                })),
                ..UsageEventData::default()
            },
        })
        .expect("record should build");

        assert_eq!(record.request_id, "req-1");
        assert_eq!(record.status, "completed");
        assert_eq!(record.billing_status, "pending");
        assert_eq!(record.total_tokens, Some(30));
        assert_eq!(
            record
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("requested_reasoning_effort"))
                .and_then(serde_json::Value::as_str),
            Some("xhigh")
        );
        assert_eq!(
            record
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("provider_reasoning_effort"))
                .and_then(serde_json::Value::as_str),
            Some("max")
        );
        assert_eq!(
            record
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("provider_service_tier"))
                .and_then(serde_json::Value::as_str),
            Some("priority")
        );
        assert_eq!(
            record
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("provider_actual_service_tier"))
                .and_then(serde_json::Value::as_str),
            Some("default")
        );
        assert_eq!(record.finalized_at_unix_secs, Some(1_700_000_000));
    }

    #[test]
    fn truncated_request_placeholders_preserve_pre_capture_request_facts() {
        let truncated = serde_json::json!({
            "truncated": true,
            "reason": "body_capture_limit_exceeded",
            "max_bytes": 128,
            "source_bytes": 2048,
            "value_kind": "object"
        });
        let record = build_upsert_usage_record_from_event(&UsageEvent {
            event_type: UsageEventType::Completed,
            request_id: "req-truncated-request-facts".to_string(),
            timestamp_ms: 1_700_000_000_000,
            data: UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                api_format: Some("openai:responses".to_string()),
                endpoint_api_format: Some("openai:responses".to_string()),
                request_body: Some(truncated.clone()),
                request_body_state: Some(UsageBodyCaptureState::Truncated),
                provider_request_body: Some(truncated),
                provider_request_body_state: Some(UsageBodyCaptureState::Truncated),
                request_metadata: Some(serde_json::json!({
                    "requested_reasoning_effort": "xhigh",
                    "provider_reasoning_effort": "max",
                    "provider_service_tier": "priority"
                })),
                ..UsageEventData::default()
            },
        })
        .expect("record should build");

        let metadata = record
            .request_metadata
            .as_ref()
            .expect("derived request facts should remain");
        assert_eq!(metadata["requested_reasoning_effort"], "xhigh");
        assert_eq!(metadata["provider_reasoning_effort"], "max");
        assert_eq!(metadata["provider_service_tier"], "priority");
    }

    #[test]
    fn disabled_reference_and_unavailable_bodies_preserve_pre_capture_request_facts() {
        for state in [
            UsageBodyCaptureState::Disabled,
            UsageBodyCaptureState::Reference,
            UsageBodyCaptureState::Unavailable,
        ] {
            let record = build_upsert_usage_record_from_event(&UsageEvent {
                event_type: UsageEventType::Completed,
                request_id: format!("req-preserved-{state:?}"),
                timestamp_ms: 1_700_000_000_000,
                data: UsageEventData {
                    provider_name: "OpenAI".to_string(),
                    model: "gpt-5".to_string(),
                    request_body_state: Some(state),
                    provider_request_body_state: Some(state),
                    request_metadata: Some(serde_json::json!({
                        "requested_reasoning_effort": "xhigh",
                        "provider_reasoning_effort": "max",
                        "provider_service_tier": "priority"
                    })),
                    ..UsageEventData::default()
                },
            })
            .expect("record should build");

            let metadata = record
                .request_metadata
                .as_ref()
                .expect("derived request facts should remain");
            assert_eq!(metadata["requested_reasoning_effort"], "xhigh");
            assert_eq!(metadata["provider_reasoning_effort"], "max");
            assert_eq!(metadata["provider_service_tier"], "priority");
        }
    }

    #[test]
    fn missing_or_complete_factless_final_bodies_clear_stale_request_facts() {
        let cases = [
            ("typed-missing", Some(UsageBodyCaptureState::None), None),
            (
                "typed-missing-with-stale-body",
                Some(UsageBodyCaptureState::None),
                Some(serde_json::json!({
                    "reasoning_effort": "xhigh",
                    "service_tier": "priority"
                })),
            ),
            ("untyped-missing", None, None),
            (
                "inline-without-facts",
                Some(UsageBodyCaptureState::Inline),
                Some(serde_json::json!({"model": "gpt-5"})),
            ),
        ];

        for (name, state, body) in cases {
            let record = build_upsert_usage_record_from_event(&UsageEvent {
                event_type: UsageEventType::Completed,
                request_id: format!("req-clear-{name}"),
                timestamp_ms: 1_700_000_000_000,
                data: UsageEventData {
                    provider_name: "OpenAI".to_string(),
                    model: "gpt-5".to_string(),
                    request_body: body.clone(),
                    request_body_state: state,
                    provider_request_body: body,
                    provider_request_body_state: state,
                    request_metadata: Some(serde_json::json!({
                        "trace_id": "trace-1",
                        "requested_reasoning_effort": "xhigh",
                        "provider_reasoning_effort": "max",
                        "provider_service_tier": "priority",
                        "provider_actual_service_tier": "priority"
                    })),
                    ..UsageEventData::default()
                },
            })
            .expect("record should build");

            let metadata = record
                .request_metadata
                .as_ref()
                .expect("audit facts remain");
            assert_eq!(metadata["trace_id"], "trace-1");
            assert_eq!(metadata["provider_actual_service_tier"], "priority");
            assert!(metadata.get("requested_reasoning_effort").is_none());
            assert!(metadata.get("provider_reasoning_effort").is_none());
            assert!(metadata.get("provider_service_tier").is_none());
        }
    }

    #[test]
    fn cancelled_terminal_record_is_void_for_billing() {
        let record = build_upsert_usage_record_from_event(&UsageEvent {
            event_type: UsageEventType::Cancelled,
            request_id: "req-cancelled".to_string(),
            timestamp_ms: 1_700_000_000_000,
            data: UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
                total_cost_usd: Some(0.03),
                actual_total_cost_usd: Some(0.02),
                status_code: Some(499),
                response_time_ms: Some(200),
                first_byte_time_ms: Some(50),
                ..UsageEventData::default()
            },
        })
        .expect("record should build");

        assert_eq!(record.status, "cancelled");
        assert_eq!(record.billing_status, "void");
        assert_eq!(record.total_tokens, Some(30));
        assert_eq!(record.total_cost_usd, Some(0.03));
        assert_eq!(record.actual_total_cost_usd, Some(0.02));
        assert_eq!(record.status_code, Some(499));
        assert_eq!(record.response_time_ms, Some(200));
        assert_eq!(record.first_byte_time_ms, Some(50));
    }

    #[test]
    fn sanitizes_request_metadata_before_building_upsert_record() {
        let record = build_upsert_usage_record_from_event(&UsageEvent {
            event_type: UsageEventType::Completed,
            request_id: "req-2".to_string(),
            timestamp_ms: 1_700_000_000_000,
            data: UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                request_metadata: Some(serde_json::json!({
                    "request_id": "req-2",
                    "provider_id": "provider-1",
                    "candidate_id": "cand-2",
                    "key_name": "upstream-primary",
                    "billing_snapshot": { "status": "complete" }
                })),
                ..UsageEventData::default()
            },
        })
        .expect("record should build");

        assert_eq!(record.candidate_id.as_deref(), Some("cand-2"));
        assert_eq!(record.key_name.as_deref(), Some("upstream-primary"));
        assert_eq!(
            record.request_metadata,
            Some(serde_json::json!({
                "billing_snapshot": { "status": "complete" }
            }))
        );
    }
}
