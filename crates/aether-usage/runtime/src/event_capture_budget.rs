use std::sync::{Arc, LazyLock};

#[cfg(test)]
use serde_json::Value;

#[doc(hidden)]
pub use aether_data_contracts::repository::usage::UsageCaptureRetention as UsageEventCaptureRetention;
pub(crate) use aether_data_contracts::repository::usage::{
    usage_json_heap_estimate as json_heap_estimate,
    UsageCaptureMemoryBudget as EventCaptureMemoryBudget,
};

const DEFAULT_CAPTURE_MEMORY_BUDGET_BYTES: usize = 128 * 1024 * 1024;

static CAPTURE_MEMORY_BUDGET: LazyLock<Arc<EventCaptureMemoryBudget>> = LazyLock::new(|| {
    let limit = std::env::var("AETHER_USAGE_EVENT_CAPTURE_MEMORY_BUDGET_BYTES")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_CAPTURE_MEMORY_BUDGET_BYTES);
    Arc::new(EventCaptureMemoryBudget::new(limit))
});

pub(crate) fn shared_capture_memory_budget() -> Arc<EventCaptureMemoryBudget> {
    Arc::clone(&CAPTURE_MEMORY_BUDGET)
}

pub(crate) fn capture_memory_metrics() -> (usize, usize, u64) {
    CAPTURE_MEMORY_BUDGET.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_capture_budget_resize_and_drop_release_estimate() {
        let budget = Arc::new(EventCaptureMemoryBudget::new(16));
        let mut retention = UsageEventCaptureRetention::default();
        assert!(retention.reserve(Arc::clone(&budget), 12));
        assert!(!retention.reserve(Arc::clone(&budget), 17));
        assert_eq!(budget.retained_bytes(), 12);
        assert_eq!(budget.downgraded_total(), 1);
        assert!(retention.reserve(Arc::clone(&budget), 4));
        assert_eq!(budget.retained_bytes(), 4);
        drop(retention);
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn event_capture_budget_unmanaged_clone_skips_estimation_and_empty_clone_is_free() {
        let unmanaged = UsageEventCaptureRetention::default();
        let (_, retained) =
            unmanaged.clone_for_bodies(|| panic!("unmanaged JSON must not be scanned"));
        assert!(retained);
        let budget = Arc::new(EventCaptureMemoryBudget::new(0));
        let mut managed = UsageEventCaptureRetention::default();
        assert!(managed.reserve(Arc::clone(&budget), 0));
        let (cloned, retained) = managed.clone_for_bodies(|| 0);
        assert!(retained);
        drop((managed, cloned));
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(budget.downgraded_total(), 0);
    }

    #[test]
    fn event_capture_budget_estimate_counts_string_and_array_spare_capacity() {
        let mut text = String::with_capacity(1024);
        text.push('x');
        let mut array = Vec::with_capacity(16);
        let expected = text.capacity() + array.capacity() * std::mem::size_of::<Value>();
        array.push(Value::String(text));
        assert_eq!(json_heap_estimate(&Value::Array(array)), expected);
    }

    #[test]
    fn event_capture_budget_parallel_owners_never_exceed_shared_limit() {
        let budget = Arc::new(EventCaptureMemoryBudget::new(1024));
        let barrier = Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let budget = Arc::clone(&budget);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    for _ in 0..100 {
                        let mut retained = UsageEventCaptureRetention::default();
                        barrier.wait();
                        let _ = retained.reserve(Arc::clone(&budget), 400);
                        barrier.wait();
                        assert_eq!(budget.retained_bytes(), 800);
                        barrier.wait();
                        drop(retained);
                        barrier.wait();
                        assert_eq!(budget.retained_bytes(), 0);
                        barrier.wait();
                    }
                });
            }
        });
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(budget.downgraded_total(), 600);
    }
}
