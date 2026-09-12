use super::*;

fn config() -> UsageRuntimeConfig {
    UsageRuntimeConfig {
        enabled: true,
        queue_terminal_events: true,
        queue_lifecycle_events: true,
        worker_count: 2,
        consumer_block_ms: 60_000,
        enqueue_retry_buffer_capacity: 256,
        enqueue_retry_initial_backoff_ms: 60_000,
        enqueue_retry_max_backoff_ms: 60_000,
        ..UsageRuntimeConfig::default()
    }
}

fn store() -> CloneQueueConfiguredUsageStore {
    CloneQueueConfiguredUsageStore {
        records: Arc::new(Mutex::new(Vec::new())),
        queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
    }
}

fn terminal(request_id: &str) -> UsageEvent {
    UsageEvent::new(
        UsageEventType::Completed,
        request_id,
        UsageEventData {
            provider_name: "openai".to_string(),
            model: "test".to_string(),
            status_code: Some(200),
            input_tokens: Some(3),
            output_tokens: Some(7),
            total_tokens: Some(10),
            ..UsageEventData::default()
        },
    )
}

#[tokio::test]
async fn usage_shutdown_waits_for_a_producer_before_closing_admission() {
    let runtime = UsageRuntime::new(config()).unwrap();
    let store = store();
    let producer = runtime.track_producer();
    let copy = runtime.clone();
    let task = tokio::spawn(async move { copy.shutdown(Duration::from_secs(3)).await });
    sleep(Duration::from_millis(30)).await;
    assert!(!task.is_finished());
    runtime
        .submit_terminal_event(&store, terminal("last-producer"))
        .await;
    drop(producer);
    task.await.unwrap().unwrap();
    assert_eq!(runtime.local_work_pending(), 0);
    assert_eq!(
        store
            .queue
            .stats(&runtime.config.stream_key, None)
            .await
            .unwrap()
            .stream_length,
        1
    );
    runtime.shutdown(Duration::from_secs(1)).await.unwrap();
    runtime
        .submit_terminal_event(&store, terminal("closed"))
        .await;
    runtime
        .record_terminal_event(&store, terminal("closed-direct-api"))
        .await;
    assert_eq!(
        store
            .queue
            .stats(&runtime.config.stream_key, None)
            .await
            .unwrap()
            .stream_length,
        1
    );
}

#[tokio::test]
async fn usage_shutdown_persists_concurrent_terminal_handoffs() {
    let runtime = UsageRuntime::new(config()).unwrap();
    let store = store();
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..128 {
        let runtime = runtime.clone();
        let store = store.clone();
        let producer = runtime.track_producer();
        tasks.spawn(async move {
            let _producer = producer;
            runtime
                .submit_terminal_event(&store, terminal(&format!("drain-{index}")))
                .await;
        });
    }
    runtime.shutdown(Duration::from_secs(5)).await.unwrap();
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(
        store
            .queue
            .stats(&runtime.config.stream_key, None)
            .await
            .unwrap()
            .stream_length,
        128
    );
    assert_eq!(runtime.local_work_pending(), 0);
}

#[tokio::test]
async fn usage_shutdown_wakes_retry_backoff_and_preserves_all_buffered_events() {
    let runtime = UsageRuntime::new(config()).unwrap();
    let store = store();
    let queue = UsageQueue::new(Arc::clone(&store.queue), runtime.config.clone()).unwrap();
    for index in 0..32 {
        assert!(runtime.enqueue_retry.schedule(
            queue.clone(),
            terminal(&format!("retry-{index}")),
            "terminal",
            DataLayerError::Redis("transient".into())
        ));
    }
    runtime.shutdown(Duration::from_secs(3)).await.unwrap();
    assert_eq!(runtime.enqueue_retry.pending(), 0);
    assert_eq!(runtime.enqueue_retry.recovered_total(), 32);
    assert_eq!(
        store
            .queue
            .stats(&runtime.config.stream_key, None)
            .await
            .unwrap()
            .stream_length,
        32
    );
}

#[tokio::test]
async fn usage_shutdown_failure_retains_retry_work_for_a_later_attempt() {
    let runtime = UsageRuntime::new(config()).unwrap();
    let store = store();
    let flaky = Arc::new(FlakyAppendQueueStore::new(
        Arc::clone(&store.queue),
        usize::MAX,
    ));
    let queue = UsageQueue::new(flaky.clone(), runtime.config.clone()).unwrap();
    assert!(runtime.enqueue_retry.schedule(
        queue,
        terminal("recover-after-deadline"),
        "terminal",
        DataLayerError::Redis("unavailable".into())
    ));
    let result = runtime.shutdown(Duration::from_millis(150)).await;
    assert!(matches!(result, Err(DataLayerError::TimedOut(_))));
    assert_eq!(runtime.enqueue_retry.pending(), 1);
    assert!(flaky.append_attempts.load(Ordering::Acquire) <= 4);
    flaky.remaining_failures.store(0, Ordering::Release);
    runtime.shutdown(Duration::from_secs(3)).await.unwrap();
    assert_eq!(runtime.enqueue_retry.recovered_total(), 1);
    assert_eq!(
        store
            .queue
            .stats(&runtime.config.stream_key, None)
            .await
            .unwrap()
            .stream_length,
        1
    );
}

#[tokio::test]
async fn usage_shutdown_flushes_delayed_lifecycle_without_waiting_for_its_timer() {
    let mut config = config();
    config.lifecycle_enqueue_delay_ms = 60_000;
    let runtime = UsageRuntime::new(config).unwrap();
    let store = store();
    let event = UsageEvent::new(
        UsageEventType::Pending,
        "delayed",
        UsageEventData::default(),
    );
    runtime
        .enqueue_lifecycle_event_with_config_delay(&store, event)
        .await;
    assert!(runtime.local_work_pending() > 0);
    runtime.shutdown(Duration::from_secs(3)).await.unwrap();
    assert_eq!(runtime.local_work_pending(), 0);
    assert_eq!(
        store
            .queue
            .stats(&runtime.config.stream_key, None)
            .await
            .unwrap()
            .stream_length,
        1
    );
}

#[tokio::test]
async fn usage_shutdown_stops_every_idle_worker_and_supervisor() {
    for supervised in [false, true] {
        let runtime = UsageRuntime::new(config()).unwrap();
        let store = Arc::new(store());
        let handles = if supervised {
            vec![runtime.spawn_worker_supervisor(store).unwrap()]
        } else {
            runtime.spawn_workers(store)
        };
        sleep(Duration::from_millis(30)).await;
        runtime.shutdown(Duration::from_secs(3)).await.unwrap();
        for handle in handles {
            timeout(Duration::from_secs(1), handle)
                .await
                .unwrap()
                .unwrap();
        }
        assert_eq!(runtime.metrics_snapshot().worker_active_count, 0);
        assert_eq!(runtime.shutdown.supervisors.load(Ordering::Acquire), 0);
    }
}

#[tokio::test]
async fn usage_shutdown_does_not_cancel_a_write_or_ack_its_unfinished_record() {
    let runtime = UsageRuntime::new(config()).unwrap();
    let store = BlockingWriteQueueConfiguredUsageStore {
        records: Arc::new(Mutex::new(Vec::new())),
        queue: store().queue,
        write_started: Arc::new(tokio::sync::Notify::new()),
        release_writes: Arc::new(tokio::sync::Notify::new()),
        writes_completed: Arc::new(AtomicUsize::new(0)),
    };
    let queue = UsageQueue::new(Arc::clone(&store.queue), runtime.config.clone()).unwrap();
    queue.ensure_consumer_group().await.unwrap();
    queue.enqueue(&terminal("in-flight-worker")).await.unwrap();
    let worker = runtime.spawn_worker(Arc::new(store.clone())).unwrap();
    timeout(Duration::from_secs(3), store.write_started.notified())
        .await
        .unwrap();
    assert!(runtime.shutdown(Duration::from_millis(50)).await.is_err());
    assert!(!worker.is_finished());
    assert_eq!(
        store
            .queue
            .stats(
                &runtime.config.stream_key,
                Some(&runtime.config.consumer_group)
            )
            .await
            .unwrap()
            .group_pending,
        1
    );
    store.release_writes.notify_one();
    runtime.shutdown(Duration::from_secs(3)).await.unwrap();
    worker.await.unwrap();
    assert_eq!(store.writes_completed.load(Ordering::Acquire), 1);
    assert_eq!(
        store
            .queue
            .stats(
                &runtime.config.stream_key,
                Some(&runtime.config.consumer_group)
            )
            .await
            .unwrap()
            .group_pending,
        0
    );
}

#[tokio::test]
async fn usage_shutdown_disabled_runtime_is_immediate() {
    UsageRuntime::disabled()
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn usage_shutdown_consumes_process_local_queue_before_stopping_workers() {
    let runtime = UsageRuntime::new(config()).unwrap();
    let store = store();
    let worker = runtime
        .spawn_worker_supervisor(Arc::new(store.clone()))
        .unwrap();
    for index in 0..64 {
        runtime
            .submit_terminal_event(&store, terminal(&format!("local-{index}")))
            .await;
    }
    runtime
        .shutdown_with_local_queue(Duration::from_secs(5), Some(Arc::clone(&store.queue)))
        .await
        .unwrap();
    worker.await.unwrap();
    assert_eq!(store.records.lock().unwrap().len(), 64);
    assert_eq!(
        store
            .queue
            .stats(
                &runtime.config.stream_key,
                Some(&runtime.config.consumer_group)
            )
            .await
            .unwrap()
            .group_pending,
        0
    );
}

#[tokio::test]
async fn usage_shutdown_rejects_unconsumed_memory_queue_as_success() {
    let runtime = UsageRuntime::new(config()).unwrap();
    let store = store();
    runtime
        .submit_terminal_event(&store, terminal("unconsumed"))
        .await;
    let result = runtime
        .shutdown_with_local_queue(Duration::from_millis(50), Some(Arc::clone(&store.queue)))
        .await;
    assert!(result.is_err());
    let worker = runtime.spawn_worker(Arc::new(store.clone())).unwrap();
    runtime
        .shutdown_with_local_queue(Duration::from_secs(3), Some(Arc::clone(&store.queue)))
        .await
        .unwrap();
    worker.await.unwrap();
    assert_eq!(store.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn usage_shutdown_does_not_miss_accepted_pending_to_terminal_handoffs() {
    let mut config = config();
    config.queue_terminal_events = false;
    let runtime = UsageRuntime::new(config).unwrap();
    let store = store();
    for index in 0..32 {
        let id = format!("ordered-{index}");
        let plan = terminal_test_plan(&id);
        runtime.record_pending(&store, build_lifecycle_usage_seed(&plan, None));
        runtime.record_stream_started(
            &store,
            &build_lifecycle_usage_seed(&plan, None),
            200,
            Some(&ExecutionTelemetry {
                ttfb_ms: Some(5),
                elapsed_ms: None,
                upstream_bytes: None,
            }),
        );
        runtime.submit_terminal_event(&store, terminal(&id)).await;
    }
    runtime.shutdown(Duration::from_secs(5)).await.unwrap();
    let records = store.records.lock().unwrap();
    for index in 0..32 {
        let statuses: Vec<_> = records
            .iter()
            .filter(|r| r.request_id == format!("ordered-{index}"))
            .map(|r| r.status.as_str())
            .collect();
        assert_eq!(statuses, ["pending", "streaming", "completed"]);
    }
}
