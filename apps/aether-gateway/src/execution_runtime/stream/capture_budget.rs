use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

const DEFAULT_STREAM_CAPTURE_MEMORY_BUDGET_BYTES: usize = 128 * 1024 * 1024;
const STREAM_CAPTURE_MEMORY_BUDGET_ENV: &str = "AETHER_GATEWAY_STREAM_CAPTURE_MEMORY_BUDGET_BYTES";

static STREAM_CAPTURE_BUDGET: LazyLock<Arc<StreamCaptureBudget>> = LazyLock::new(|| {
    StreamCaptureBudget::new(
        std::env::var(STREAM_CAPTURE_MEMORY_BUDGET_ENV)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(DEFAULT_STREAM_CAPTURE_MEMORY_BUDGET_BYTES),
    )
});

#[derive(Debug)]
pub(super) struct StreamCaptureBudget {
    available: AtomicUsize,
}

impl StreamCaptureBudget {
    pub(super) fn new(bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            available: AtomicUsize::new(bytes),
        })
    }

    fn reserve_up_to(&self, wanted: usize, minimum: usize) -> usize {
        let mut available = self.available.load(Ordering::Relaxed);
        loop {
            let reserved = wanted.min(available);
            if reserved < minimum {
                return 0;
            }
            match self.available.compare_exchange_weak(
                available,
                available - reserved,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return reserved,
                Err(current) => available = current,
            }
        }
    }

    fn release(&self, bytes: usize) {
        self.available.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Only retained diagnostic bytes belong here. Protocol and billing observers
/// must consume the original chunks independently of capture admission.
#[derive(Debug)]
pub(super) struct StreamBodyCapture {
    bytes: Vec<u8>,
    budget: Arc<StreamCaptureBudget>,
}

impl Default for StreamBodyCapture {
    fn default() -> Self {
        Self::with_budget(Arc::clone(&STREAM_CAPTURE_BUDGET))
    }
}

impl StreamBodyCapture {
    pub(super) fn with_budget(budget: Arc<StreamCaptureBudget>) -> Self {
        Self {
            bytes: Vec::new(),
            budget,
        }
    }

    pub(super) fn append(&mut self, chunk: &[u8], limit: usize, truncated: &mut bool) {
        if chunk.is_empty() || *truncated {
            return;
        }
        let wanted_len = self.bytes.len().saturating_add(chunk.len()).min(limit);
        if wanted_len > self.bytes.capacity() {
            // Keep the old allocation charged until its replacement has been
            // allocated and copied, including their overlap during growth.
            let wanted_capacity = wanted_len
                .max(self.bytes.capacity().saturating_mul(2))
                .min(limit);
            let reserved = self
                .budget
                .reserve_up_to(wanted_capacity, self.bytes.capacity().saturating_add(1));
            if reserved > 0 {
                let mut replacement = Vec::new();
                if replacement.try_reserve_exact(reserved).is_ok() {
                    let extra = replacement.capacity().saturating_sub(reserved);
                    if extra == 0 || self.budget.reserve_up_to(extra, extra) == extra {
                        replacement.extend_from_slice(&self.bytes);
                        let old = std::mem::replace(&mut self.bytes, replacement);
                        let old_capacity = old.capacity();
                        drop(old);
                        self.budget.release(old_capacity);
                    } else {
                        drop(replacement);
                        self.budget.release(reserved);
                    }
                } else {
                    self.budget.release(reserved);
                }
            }
        }
        let keep = wanted_len
            .min(self.bytes.capacity())
            .saturating_sub(self.bytes.len());
        self.bytes.extend_from_slice(&chunk[..keep]);
        // Once bytes are omitted, never append a later suffix to this prefix.
        *truncated = keep < chunk.len();
    }
}

impl Deref for StreamBodyCapture {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl Drop for StreamBodyCapture {
    fn drop(&mut self) {
        let bytes = std::mem::take(&mut self.bytes);
        let capacity = bytes.capacity();
        drop(bytes);
        self.budget.release(capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_capture_budget_is_shared_and_released_on_drop() {
        let budget = StreamCaptureBudget::new(12);
        let mut provider = StreamBodyCapture::with_budget(Arc::clone(&budget));
        let mut client = StreamBodyCapture::with_budget(Arc::clone(&budget));
        let mut provider_truncated = false;
        let mut client_truncated = false;
        provider.append(b"12345678", 64, &mut provider_truncated);
        client.append(b"abcdefgh", 64, &mut client_truncated);
        assert_eq!(&*provider, b"12345678");
        assert_eq!(&*client, b"abcd");
        assert!(!provider_truncated);
        assert!(client_truncated);
        assert_eq!(budget.available.load(Ordering::Relaxed), 0);
        drop(provider);
        client.append(b"later", 64, &mut client_truncated);
        assert_eq!(&*client, b"abcd");
        drop(client);
        assert_eq!(budget.available.load(Ordering::Relaxed), 12);
    }

    #[test]
    fn stream_capture_budget_charges_capacity_and_reallocation_overlap() {
        let budget = StreamCaptureBudget::new(16);
        let mut capture = StreamBodyCapture::with_budget(Arc::clone(&budget));
        let mut truncated = false;
        capture.append(b"1234", 64, &mut truncated);
        capture.append(b"5", 64, &mut truncated);
        assert_eq!(capture.bytes.capacity(), 8);
        assert_eq!(budget.available.load(Ordering::Relaxed), 8);
        capture.append(b"6789", 64, &mut truncated);
        assert_eq!(&*capture, b"12345678");
        assert!(truncated);
        assert_eq!(budget.available.load(Ordering::Relaxed), 8);
        drop(capture);
        assert_eq!(budget.available.load(Ordering::Relaxed), 16);
    }

    #[test]
    fn stream_capture_budget_zero_disables_capture_without_allocating() {
        let budget = StreamCaptureBudget::new(0);
        let mut capture = StreamBodyCapture::with_budget(budget);
        let mut truncated = false;
        capture.append(b"data", 64, &mut truncated);
        assert!(capture.is_empty());
        assert_eq!(capture.bytes.capacity(), 0);
        assert!(truncated);
    }

    #[test]
    fn stream_capture_budget_exhaustion_uses_existing_spare_capacity() {
        let budget = StreamCaptureBudget::new(14);
        let mut capture = StreamBodyCapture::with_budget(budget);
        let mut truncated = false;
        capture.append(b"1234", 64, &mut truncated);
        capture.append(b"5", 64, &mut truncated);
        assert_eq!(capture.bytes.capacity(), 8);
        capture.append(b"6789", 64, &mut truncated);
        assert_eq!(&*capture, b"12345678");
        assert_eq!(capture.bytes.capacity(), 8);
        assert!(truncated);
    }

    #[test]
    fn stream_capture_budget_local_limit_keeps_a_contiguous_prefix() {
        let budget = StreamCaptureBudget::new(128);
        let mut capture = StreamBodyCapture::with_budget(Arc::clone(&budget));
        let mut truncated = false;
        capture.append(b"abcdef", 3, &mut truncated);
        assert_eq!(&*capture, b"abc");
        assert!(truncated);
        assert_eq!(budget.available.load(Ordering::Relaxed), 125);
    }

    #[test]
    fn stream_capture_budget_concurrent_growth_and_drop_never_exceeds_capacity() {
        const LIMIT: usize = 256;
        const THREADS: usize = 8;
        let budget = StreamCaptureBudget::new(LIMIT);
        let barrier = std::sync::Barrier::new(THREADS);
        let held = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for index in 0..THREADS {
                let budget = &budget;
                let barrier = &barrier;
                let held = &held;
                scope.spawn(move || {
                    for _ in 0..32 {
                        let mut capture = StreamBodyCapture::with_budget(Arc::clone(budget));
                        let mut truncated = false;
                        barrier.wait();
                        capture.append(&[1; 16], LIMIT, &mut truncated);
                        capture.append(&[2; 48], LIMIT, &mut truncated);
                        held.fetch_add(capture.bytes.capacity(), Ordering::SeqCst);
                        barrier.wait();
                        if index == 0 {
                            let retained = held.load(Ordering::SeqCst);
                            assert!(retained <= LIMIT);
                            assert_eq!(retained + budget.available.load(Ordering::Relaxed), LIMIT,);
                        }
                        barrier.wait();
                        drop(capture);
                        barrier.wait();
                        if index == 0 {
                            assert_eq!(budget.available.load(Ordering::Relaxed), LIMIT);
                            held.store(0, Ordering::SeqCst);
                        }
                        barrier.wait();
                    }
                });
            }
        });
    }
}
