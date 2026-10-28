use super::*;
use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeSemaphoreConfig, RuntimeState};
use futures_util::poll;

fn gate(runtime: &RuntimeState, key: &str) -> RuntimeSemaphore {
    runtime
        .keyed_semaphore("queue_test", key, 1, RuntimeSemaphoreConfig::default())
        .unwrap()
}

async fn release(permits: Vec<RuntimeSemaphorePermit>) {
    for permit in permits {
        permit.release().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn queued_requests_acquire_in_arrival_order() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let gates = [gate(&runtime, "fifo")];
    let queue = ProviderConcurrencyQueue::new(Duration::from_secs(10), 3);
    let held = queue.acquire(&gates, "fifo").await.unwrap();
    let mut first = Box::pin(queue.acquire(&gates, "fifo"));
    let mut second = Box::pin(queue.acquire(&gates, "fifo"));
    assert!(poll!(&mut first).is_pending());
    assert!(poll!(&mut second).is_pending());

    release(held).await;
    let first_permits = first.await.unwrap();

    assert!(poll!(&mut second).is_pending());
    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 1);
    release(first_permits).await;
    release(second.await.unwrap()).await;
}

#[tokio::test(start_paused = true)]
async fn key_queue_opt_out_rejects_saturation_without_waiting_or_leaking_permits() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let gates = [gate(&runtime, "key"), gate(&runtime, "provider")];
    let queue = ProviderConcurrencyQueue::new(Duration::from_secs(10), 3);
    for saturated_index in [0, 1] {
        let held = gates[saturated_index].try_acquire().await.unwrap();
        let started = Instant::now();
        let rejected = queue.acquire_observed(&gates, "key", false, None).await;
        assert!(matches!(
            rejected,
            Err(RuntimeSemaphoreError::Saturated { .. })
        ));
        assert_eq!(Instant::now(), started);
        assert!(!queue.is_waiting("key"));
        assert_eq!(
            gates[1 - saturated_index]
                .snapshot()
                .await
                .unwrap()
                .in_flight,
            0
        );
        held.release().await.unwrap();
    }
    release(
        queue
            .acquire_observed(&gates, "key", false, None)
            .await
            .unwrap(),
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn full_queue_rejects_and_cancellation_returns_waiting_capacity() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let gates = [gate(&runtime, "full")];
    let queue = ProviderConcurrencyQueue::new(Duration::from_secs(10), 1);
    let held = queue.acquire(&gates, "full").await.unwrap();
    let mut waiting = Box::pin(queue.acquire(&gates, "full"));
    assert!(poll!(&mut waiting).is_pending());

    let rejected = queue.acquire(&gates, "full").await;

    assert!(matches!(
        rejected,
        Err(RuntimeSemaphoreError::Saturated { .. })
    ));
    drop(waiting);
    let mut replacement = Box::pin(queue.acquire(&gates, "full"));
    assert!(poll!(&mut replacement).is_pending());
    release(held).await;
    release(replacement.await.unwrap()).await;
}

#[tokio::test(start_paused = true)]
async fn timed_out_waiter_leaves_runtime_capacity_unchanged() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let gates = [gate(&runtime, "timeout")];
    let queue = ProviderConcurrencyQueue::new(Duration::from_secs(1), 1);
    let held = queue.acquire(&gates, "timeout").await.unwrap();

    let result = queue.acquire(&gates, "timeout").await;

    assert!(matches!(
        result,
        Err(RuntimeSemaphoreError::Saturated { .. })
    ));
    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 1);
    release(held).await;
    release(queue.acquire(&gates, "timeout").await.unwrap()).await;
}

#[tokio::test(start_paused = true)]
async fn group_admission_rolls_back_without_blocking_other_credentials() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let gates = [gate(&runtime, "key"), gate(&runtime, "provider")];
    let held_provider = gates[1].try_acquire().await.unwrap();
    let queue = ProviderConcurrencyQueue::new(Duration::from_secs(10), 2);
    let mut waiting = Box::pin(queue.acquire(&gates, "key"));

    assert!(poll!(&mut waiting).is_pending());

    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 0);
    let independent = [gate(&runtime, "independent")];
    release(queue.acquire(&independent, "independent").await.unwrap()).await;
    held_provider.release().await.unwrap();
    release(waiting.await.unwrap()).await;
}

#[tokio::test(start_paused = true)]
async fn disabled_queue_attempts_admission_once_without_waiting() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let gates = [gate(&runtime, "disabled")];
    let queue = ProviderConcurrencyQueue::new(Duration::ZERO, 1);
    let held = queue.acquire(&gates, "disabled").await.unwrap();
    let started = Instant::now();

    let result = queue.acquire(&gates, "disabled").await;

    assert!(matches!(
        result,
        Err(RuntimeSemaphoreError::Saturated { .. })
    ));
    assert_eq!(started.elapsed(), Duration::ZERO);
    release(held).await;
}

#[tokio::test(start_paused = true)]
async fn independent_queues_still_share_runtime_capacity() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let gates = [gate(&runtime, "shared")];
    let first_queue = ProviderConcurrencyQueue::new(Duration::from_secs(10), 1);
    let second_queue = ProviderConcurrencyQueue::new(Duration::from_secs(10), 1);
    let held = first_queue.acquire(&gates, "shared").await.unwrap();
    let mut waiting = Box::pin(second_queue.acquire(&gates, "shared"));
    assert!(poll!(&mut waiting).is_pending());
    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 1);
    release(held).await;
    release(waiting.await.unwrap()).await;
    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 0);
}

#[tokio::test(start_paused = true)]
async fn timeout_reports_the_gate_that_is_actually_saturated() {
    let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
    let key = gate(&runtime, "scope-key");
    let provider = runtime
        .keyed_semaphore(
            "provider",
            "scope-provider",
            1,
            RuntimeSemaphoreConfig::default(),
        )
        .unwrap();
    let held = provider.try_acquire().await.unwrap();
    let queue = ProviderConcurrencyQueue::new(Duration::from_millis(100), 1);
    let result = queue.acquire(&[key, provider], "scope-key").await;
    assert!(matches!(
        result,
        Err(RuntimeSemaphoreError::Saturated {
            gate: "provider",
            limit: 1
        })
    ));
    held.release().await.unwrap();
}
