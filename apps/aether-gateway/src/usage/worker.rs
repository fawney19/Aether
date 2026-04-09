pub(crate) use aether_usage_runtime::{build_usage_queue_worker, write_event_record};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::usage::UsageReadRepository;

    use super::write_event_record;
    use crate::data::GatewayDataState;
    use crate::usage::{UsageEvent, UsageEventData, UsageEventType};

    #[tokio::test]
    async fn worker_writes_usage_record_from_terminal_event() {
        let repository = Arc::new(InMemoryUsageReadRepository::default());
        let data = GatewayDataState::with_usage_repository_for_tests(repository.clone());
        let event = UsageEvent::new(
            UsageEventType::Completed,
            "req-worker-123".to_string(),
            UsageEventData {
                user_id: Some("user-worker-123".to_string()),
                api_key_id: Some("api-key-worker-123".to_string()),
                provider_name: "openai".to_string(),
                provider_id: Some("provider-worker-123".to_string()),
                provider_endpoint_id: Some("endpoint-worker-123".to_string()),
                provider_api_key_id: Some("provider-key-worker-123".to_string()),
                model: "gpt-5".to_string(),
                api_format: Some("openai:chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                is_stream: Some(false),
                status_code: Some(200),
                input_tokens: Some(4),
                output_tokens: Some(6),
                total_tokens: Some(10),
                response_time_ms: Some(52),
                ..UsageEventData::default()
            },
        );

        write_event_record(&data, &event)
            .await
            .expect("worker should write usage record");

        let stored = repository
            .find_by_request_id("req-worker-123")
            .await
            .expect("usage lookup should succeed")
            .expect("usage record should exist");

        assert_eq!(stored.status, "completed");
        assert_eq!(stored.billing_status, "pending");
        assert_eq!(stored.total_tokens, 10);
        assert_eq!(stored.response_time_ms, Some(52));
        assert_eq!(stored.user_id.as_deref(), Some("user-worker-123"));
    }
}
