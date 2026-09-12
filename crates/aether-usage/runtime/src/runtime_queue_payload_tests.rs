use super::*;

use super::super::{run_usage_enqueue_retry_worker, TerminalPersistenceOutcome};

const PAYLOAD_LIMIT: usize = 4 * 1024;

fn payload_config(name: &str) -> UsageRuntimeConfig {
    UsageRuntimeConfig {
        enabled: true,
        queue_terminal_events: true,
        queue_lifecycle_events: true,
        stream_key: format!("usage:events:test:payload:{name}"),
        consumer_group: format!("usage_consumers_payload_{name}"),
        queue_payload_max_bytes: PAYLOAD_LIMIT,
        consumer_block_ms: 1,
        enqueue_retry_buffer_capacity: 8,
        enqueue_retry_workers: 1,
        enqueue_retry_initial_backoff_ms: 1,
        enqueue_retry_max_backoff_ms: 2,
        ..UsageRuntimeConfig::default()
    }
}

fn payload_event(request_id: &str, oversized: bool) -> UsageEvent {
    UsageEvent {
        event_type: UsageEventType::Completed,
        request_id: request_id.to_string(),
        timestamp_ms: 123_000,
        data: UsageEventData {
            user_id: Some("user-payload".to_string()),
            api_key_id: Some("key-payload".to_string()),
            provider_name: "openai".to_string(),
            provider_id: Some("provider-payload".to_string()),
            provider_api_key_id: Some("provider-key-payload".to_string()),
            // Model identity is a core field, so omitting diagnostic bodies cannot make this fit.
            model: if oversized {
                "m".repeat(PAYLOAD_LIMIT * 2)
            } else {
                "gpt-5".to_string()
            },
            target_model: Some("gpt-5".to_string()),
            api_format: Some("openai:responses".to_string()),
            endpoint_api_format: Some("openai:responses".to_string()),
            input_tokens: Some(100),
            output_tokens: Some(25),
            total_tokens: Some(125),
            cache_creation_input_tokens: Some(30),
            cache_creation_ephemeral_5m_input_tokens: Some(0),
            cache_creation_ephemeral_1h_input_tokens: Some(30),
            cache_read_input_tokens: Some(0),
            total_cost_usd: Some(0.5),
            actual_total_cost_usd: Some(0.25),
            status_code: Some(200),
            error_message: Some("preserve error presence and text".to_string()),
            first_byte_time_ms: Some(12),
            response_time_ms: Some(34),
            request_headers: Some(json!({"x-request": "original"})),
            provider_request_headers: Some(json!({"x-provider-request": "original"})),
            response_headers: Some(json!({"x-provider-response": "original"})),
            client_response_headers: Some(json!({"x-client-response": "original"})),
            request_body: Some(json!({"reasoning": {"effort": "high"}, "input": "original"})),
            request_body_state: Some(UsageBodyCaptureState::Inline),
            provider_request_body: Some(json!({
                "model": "gpt-5", "service_tier": "priority",
                "prompt_cache_retention": "24h", "input": "original provider input"
            })),
            provider_request_body_state: Some(UsageBodyCaptureState::Inline),
            response_body: Some(json!({"service_tier": "default", "output": "original output"})),
            response_body_state: Some(UsageBodyCaptureState::Inline),
            client_response_body: Some(json!({"output": "original client output"})),
            client_response_body_state: Some(UsageBodyCaptureState::Inline),
            request_metadata: Some(json!({
                "api_key_is_standalone": true,
                "plan_usage_reservation_token": "550e8400-e29b-41d4-a716-446655440000",
                "plan_usage_reservation_deferred": true,
                "usage_available": true,
                "usage_pricing_available": true,
                "provider_cache_ttl_minutes": 1440,
                "provider_service_tier": "priority",
                "provider_actual_service_tier": "default",
                "dimensions": {
                    "image_count": 2, "image_size": "1024x1024", "image_quality": "high",
                    "image_output_format": "png", "reasoning_tokens": 0
                }
            })),
            ..UsageEventData::default()
        },
    }
}

async fn payload_queue(config: &UsageRuntimeConfig) -> (UsageQueue, Arc<FlakyAppendQueueStore>) {
    let inner: Arc<dyn RuntimeQueueStore> =
        Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
    let runner = Arc::new(FlakyAppendQueueStore::new(inner, 0));
    let queue = UsageQueue::new(runner.clone(), config.clone()).expect("payload queue");
    queue.ensure_consumer_group().await.expect("payload group");
    (queue, runner)
}

#[tokio::test]
async fn terminal_oversize_uses_original_event_for_direct_fallback_without_opening_circuit() {
    let config = payload_config("terminal_direct");
    let (queue, runner) = payload_queue(&config).await;
    let store = CloneQueueConfiguredUsageStore {
        records: Arc::new(Mutex::new(Vec::new())),
        queue: runner.clone(),
    };
    let runtime = UsageRuntime::new(config).expect("usage runtime");
    let event = payload_event("payload-terminal-direct", true);
    let expected = crate::build_upsert_usage_record_from_event(&event).expect("original record");

    assert!(matches!(
        queue.enqueue(&event).await,
        Err(DataLayerError::InvalidInput(_))
    ));
    assert_eq!(runner.append_attempts.load(Ordering::Acquire), 0);
    assert_eq!(
        runtime.enqueue_or_write_terminal(&store, event).await,
        TerminalPersistenceOutcome::PersistedDirectly
    );
    {
        let records = store.records.lock().expect("records lock");
        assert_eq!(records.as_slice(), &[expected]);
        assert_eq!(records[0].cache_read_input_tokens, Some(0));
        assert_eq!(records[0].cache_creation_ephemeral_5m_input_tokens, Some(0));
        assert_eq!(
            records[0].request_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
        assert!(records[0].provider_request_body.is_some());
        assert!(records[0].request_headers.is_some());
    }
    assert_eq!(
        runtime
            .terminal_enqueue_state
            .circuit_open_until_unix_ms
            .load(Ordering::Acquire),
        0
    );
    let snapshot = runtime.metrics_snapshot();
    assert_eq!(snapshot.terminal_direct_fallback_succeeded_total, 1);
    assert_eq!(snapshot.enqueue_retry_scheduled_total, 0);
    assert_eq!(snapshot.enqueue_retry_pending, 0);

    assert_eq!(
        runtime
            .enqueue_or_write_terminal(&store, payload_event("payload-after-direct", false))
            .await,
        TerminalPersistenceOutcome::Queued
    );
    assert_eq!(runner.successful_appends.load(Ordering::Acquire), 1);
    assert_eq!(store.records.lock().expect("records lock").len(), 1);
}

#[tokio::test]
async fn terminal_oversize_direct_failure_preserves_first_byte_and_does_not_retry_or_open_circuit()
{
    let config = payload_config("terminal_failed");
    let (_, runner) = payload_queue(&config).await;
    let store = FailingWriteQueueConfiguredUsageStore {
        queue: runner.clone(),
        upsert_attempts: Arc::new(AtomicUsize::new(0)),
    };
    let runtime = UsageRuntime::new(config).expect("usage runtime");
    let request_id = "payload-terminal-failed";
    let generation = runtime
        .lifecycle_coalescer
        .mark_first_byte(request_id)
        .await
        .expect("first byte");

    assert_eq!(
        runtime
            .enqueue_or_write_terminal(&store, payload_event(request_id, true))
            .await,
        TerminalPersistenceOutcome::Failed
    );
    assert!(
        runtime
            .lifecycle_coalescer
            .first_byte_is_current(request_id, generation)
            .await
    );
    assert_eq!(runner.append_attempts.load(Ordering::Acquire), 0);
    assert_eq!(store.upsert_attempts.load(Ordering::Acquire), 1);
    assert_eq!(
        runtime
            .terminal_enqueue_state
            .circuit_open_until_unix_ms
            .load(Ordering::Acquire),
        0
    );
    let snapshot = runtime.metrics_snapshot();
    assert_eq!(snapshot.terminal_direct_fallback_failed_total, 1);
    assert_eq!(snapshot.terminal_enqueue_deferred_dropped_total, 1);
    assert_eq!(snapshot.enqueue_retry_scheduled_total, 0);
    assert_eq!(snapshot.enqueue_retry_pending, 0);

    assert_eq!(
        runtime
            .enqueue_or_write_terminal(&store, payload_event("payload-after-failed", false))
            .await,
        TerminalPersistenceOutcome::Queued
    );
    assert_eq!(runner.successful_appends.load(Ordering::Acquire), 1);
    assert_eq!(store.upsert_attempts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn terminal_oversize_direct_failure_stays_failed_when_primary_enqueue_is_deferred() {
    for (name, circuit_open) in [("circuit_open", true), ("in_flight_limit", false)] {
        let mut config = payload_config(name);
        config.terminal_enqueue_max_in_flight = 1;
        let (_, runner) = payload_queue(&config).await;
        let store = FailingWriteQueueConfiguredUsageStore {
            queue: runner.clone(),
            upsert_attempts: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime");
        let request_id = format!("payload-terminal-{name}");
        let generation = runtime
            .lifecycle_coalescer
            .mark_first_byte(&request_id)
            .await
            .expect("first byte");
        let original_deadline = if circuit_open {
            let deadline = super::super::now_unix_ms().saturating_add(60_000);
            runtime.terminal_enqueue_state.open_circuit(deadline);
            deadline
        } else {
            0
        };
        let held_guard = if circuit_open {
            None
        } else {
            Some(
                runtime
                    .terminal_enqueue_state
                    .try_acquire_in_flight(1)
                    .expect("hold the only enqueue slot"),
            )
        };

        assert_eq!(
            timeout(
                Duration::from_secs(2),
                runtime.enqueue_or_write_terminal(&store, payload_event(&request_id, true)),
            )
            .await
            .expect("bounded terminal fallback"),
            TerminalPersistenceOutcome::Failed,
            "{name} must not report an oversized event as buffered"
        );
        assert!(
            runtime
                .lifecycle_coalescer
                .first_byte_is_current(&request_id, generation)
                .await
        );
        assert_eq!(runner.append_attempts.load(Ordering::Acquire), 0);
        assert_eq!(store.upsert_attempts.load(Ordering::Acquire), 1);
        assert_eq!(
            runtime
                .terminal_enqueue_state
                .circuit_open_until_unix_ms
                .load(Ordering::Acquire),
            original_deadline
        );
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.terminal_direct_fallback_failed_total, 1);
        assert_eq!(snapshot.terminal_enqueue_deferred_dropped_total, 1);
        assert_eq!(snapshot.terminal_enqueue_deferred_retry_total, 0);
        assert_eq!(snapshot.enqueue_retry_permanent_failure_total, 1);
        assert_eq!(snapshot.enqueue_retry_scheduled_total, 0);
        assert_eq!(snapshot.enqueue_retry_pending, 0);
        assert_eq!(
            runtime.terminal_enqueue_state.in_flight(),
            u64::from(!circuit_open)
        );
        drop(held_guard);
        assert_eq!(runtime.terminal_enqueue_state.in_flight(), 0);
    }
}

#[tokio::test]
async fn retry_worker_discards_oversize_and_drains_next_event_on_the_same_shard() {
    let config = payload_config("retry_drain");
    let (queue, runner) = payload_queue(&config).await;
    let (sender, receiver) = mpsc::channel(2);
    let dispatcher = UsageEnqueueRetryDispatcher {
        senders: vec![sender],
        metrics: Arc::new(Default::default()),
    };
    let metrics_view = UsageEnqueueRetryDispatcher {
        senders: Vec::new(),
        metrics: Arc::clone(&dispatcher.metrics),
    };
    for (request_id, oversized) in [
        ("payload-retry-oversize", true),
        ("payload-retry-small", false),
    ] {
        // Bypass admission to exercise the worker's defense for an already buffered event.
        assert!(dispatcher
            .schedule_item(
                queue.clone(),
                payload_event(request_id, oversized),
                "terminal",
                Some("prior transient failure"),
            )
            .is_some());
    }
    assert_eq!(dispatcher.pending(), 2);
    drop(dispatcher);

    // Run the real worker as a cancellable future, so a regression cannot leave a detached retry.
    timeout(
        Duration::from_secs(2),
        run_usage_enqueue_retry_worker(0, config, receiver, Arc::clone(&metrics_view.metrics)),
    )
    .await
    .expect("the permanent failure must not block the shard");

    assert_eq!(metrics_view.permanent_failure_total(), 1);
    assert_eq!(metrics_view.recovered_total(), 1);
    assert_eq!(metrics_view.pending(), 0);
    assert_eq!(runner.append_attempts.load(Ordering::Acquire), 1);
    let entries = queue
        .read_group("payload-retry-reader")
        .await
        .expect("queue read");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued event")
            .request_id,
        "payload-retry-small"
    );
}

#[tokio::test]
async fn retry_dispatcher_rejects_oversize_for_permanent_and_transient_causes_without_consuming_slots(
) {
    let config = payload_config("retry_reject");
    let (queue, runner) = payload_queue(&config).await;
    let (sender, receiver) = mpsc::channel(1);
    let dispatcher = UsageEnqueueRetryDispatcher {
        senders: vec![sender],
        metrics: Arc::new(Default::default()),
    };
    let oversized = payload_event("payload-rejected", true);
    let error = queue.enqueue(&oversized).await.expect_err("oversize input");
    assert!(matches!(error, DataLayerError::InvalidInput(_)));
    assert!(!dispatcher.schedule(queue.clone(), oversized, "terminal", error));
    for cause in [
        DataLayerError::TimedOut("primary enqueue was deferred".to_string()),
        DataLayerError::Redis("prior transient failure".to_string()),
    ] {
        assert!(!dispatcher.schedule(
            queue.clone(),
            payload_event("payload-rejected-transient-cause", true),
            "terminal",
            cause,
        ));
    }
    assert_eq!(dispatcher.permanent_failure_total(), 3);
    assert_eq!(dispatcher.pending(), 0);
    assert_eq!(dispatcher.scheduled_total(), 0);
    assert!(dispatcher.schedule(
        queue,
        payload_event("payload-after-reject", false),
        "terminal",
        DataLayerError::Redis("retryable failure".to_string()),
    ));
    let metrics_view = UsageEnqueueRetryDispatcher {
        senders: Vec::new(),
        metrics: Arc::clone(&dispatcher.metrics),
    };
    drop(dispatcher);

    timeout(
        Duration::from_secs(2),
        run_usage_enqueue_retry_worker(0, config, receiver, Arc::clone(&metrics_view.metrics)),
    )
    .await
    .expect("retry worker drain");

    assert_eq!(metrics_view.permanent_failure_total(), 3);
    assert_eq!(metrics_view.recovered_total(), 1);
    assert_eq!(metrics_view.pending(), 0);
    assert_eq!(runner.append_attempts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn lifecycle_oversize_does_not_open_circuit_or_block_the_next_lifecycle_event() {
    let config = payload_config("lifecycle");
    let (queue, runner) = payload_queue(&config).await;
    let store = CloneQueueConfiguredUsageStore {
        records: Arc::new(Mutex::new(Vec::new())),
        queue: runner.clone(),
    };
    let runtime = UsageRuntime::new(config).expect("usage runtime");
    let mut oversized = payload_event("payload-lifecycle-oversize", true);
    oversized.event_type = UsageEventType::Streaming;

    assert!(!runtime.enqueue_lifecycle_event(&store, oversized).await);
    assert_eq!(
        runtime
            .lifecycle_enqueue_state
            .circuit_open_until_unix_ms
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(runner.append_attempts.load(Ordering::Acquire), 0);
    let snapshot = runtime.metrics_snapshot();
    assert_eq!(snapshot.enqueue_retry_scheduled_total, 0);
    assert_eq!(snapshot.enqueue_retry_pending, 0);
    assert!(store.records.lock().expect("records lock").is_empty());

    let mut small = payload_event("payload-lifecycle-small", false);
    small.event_type = UsageEventType::Streaming;
    assert!(runtime.enqueue_lifecycle_event(&store, small).await);
    assert_eq!(runner.successful_appends.load(Ordering::Acquire), 1);
    let entries = queue
        .read_group("payload-lifecycle-reader")
        .await
        .expect("queue read");
    assert_eq!(entries.len(), 1);
    let queued = UsageEvent::from_stream_fields(&entries[0].fields).expect("queued lifecycle");
    assert_eq!(queued.request_id, "payload-lifecycle-small");
    assert_eq!(queued.event_type, UsageEventType::Streaming);
    assert_eq!(queued.data.first_byte_time_ms, Some(12));
}
