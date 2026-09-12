use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use aether_data_contracts::DataLayerError;
use aether_runtime_state::RuntimeQueueEntry;
use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const DEFAULT_ENCODING_BUDGET_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_ENCODING_JOBS: usize = 4;
const MAX_ENCODING_JOBS: usize = 128;

static ENCODING_BUDGET: LazyLock<Arc<DeadLetterEncodingBudget>> = LazyLock::new(|| {
    Arc::new(DeadLetterEncodingBudget::new(
        configured_limit(
            std::env::var("AETHER_USAGE_DLQ_ENCODING_BUDGET_BYTES")
                .ok()
                .as_deref(),
            DEFAULT_ENCODING_BUDGET_BYTES,
            maximum_budget_bytes(),
        ),
        configured_limit(
            std::env::var("AETHER_USAGE_DLQ_ENCODING_MAX_JOBS")
                .ok()
                .as_deref(),
            DEFAULT_ENCODING_JOBS,
            MAX_ENCODING_JOBS,
        ),
    ))
});

pub(crate) fn shared_dead_letter_encoding_budget() -> Arc<DeadLetterEncodingBudget> {
    Arc::clone(&ENCODING_BUDGET)
}

pub(crate) fn dead_letter_encoding_metrics() -> DeadLetterEncodingSnapshot {
    ENCODING_BUDGET.snapshot()
}

fn maximum_budget_bytes() -> usize {
    Semaphore::MAX_PERMITS.min(u32::MAX as usize)
}

fn configured_limit(raw: Option<&str>, fallback: usize, maximum: usize) -> usize {
    raw.and_then(|raw| raw.trim().parse::<u128>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(maximum as u128) as usize)
        .unwrap_or(fallback.min(maximum))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeadLetterEncodingSnapshot {
    pub(crate) limit_bytes: usize,
    pub(crate) job_limit: usize,
    pub(crate) reserved_bytes: usize,
    pub(crate) active_jobs: usize,
    pub(crate) capacity_rejected_total: u64,
    pub(crate) oversized_rejected_total: u64,
    pub(crate) encoded_total: u64,
}

/// Reserves raw string lengths plus their worst-case JSON encoding before any
/// clone or encoding allocation. This excludes collection/allocation overhead and
/// Redis command, packed-command, and connection buffers; it is not an RSS limit.
pub(crate) struct DeadLetterEncodingBudget {
    limit_bytes: usize,
    job_limit: usize,
    bytes: Arc<Semaphore>,
    jobs: Arc<Semaphore>,
    reserved_bytes: AtomicUsize,
    active_jobs: AtomicUsize,
    capacity_rejected_total: AtomicU64,
    oversized_rejected_total: AtomicU64,
    encoded_total: AtomicU64,
    #[cfg(test)]
    encode_hook: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl DeadLetterEncodingBudget {
    pub(crate) fn new(limit_bytes: usize, job_limit: usize) -> Self {
        let limit_bytes = limit_bytes.min(maximum_budget_bytes());
        let job_limit = job_limit.clamp(1, MAX_ENCODING_JOBS);
        Self {
            limit_bytes,
            job_limit,
            bytes: Arc::new(Semaphore::new(limit_bytes)),
            jobs: Arc::new(Semaphore::new(job_limit)),
            reserved_bytes: AtomicUsize::new(0),
            active_jobs: AtomicUsize::new(0),
            capacity_rejected_total: AtomicU64::new(0),
            oversized_rejected_total: AtomicU64::new(0),
            encoded_total: AtomicU64::new(0),
            #[cfg(test)]
            encode_hook: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn snapshot(&self) -> DeadLetterEncodingSnapshot {
        DeadLetterEncodingSnapshot {
            limit_bytes: self.limit_bytes,
            job_limit: self.job_limit,
            reserved_bytes: self.reserved_bytes.load(Ordering::Relaxed),
            active_jobs: self.active_jobs.load(Ordering::Relaxed),
            capacity_rejected_total: self.capacity_rejected_total.load(Ordering::Relaxed),
            oversized_rejected_total: self.oversized_rejected_total.load(Ordering::Relaxed),
            encoded_total: self.encoded_total.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn try_reserve(
        self: &Arc<Self>,
        entry: &RuntimeQueueEntry,
        error: &str,
    ) -> Result<DeadLetterEncodingReservation, DataLayerError> {
        let size = encoding_size(entry, error).filter(|size| size.total <= self.limit_bytes);
        let Some(size) = size else {
            self.oversized_rejected_total
                .fetch_add(1, Ordering::Relaxed);
            return Err(DataLayerError::InvalidInput(format!(
                "dead-letter raw fields and worst-case JSON exceed the {}-byte encoding budget",
                self.limit_bytes
            )));
        };
        let job_permit = Arc::clone(&self.jobs)
            .try_acquire_owned()
            .map_err(|_| self.capacity_error())?;
        let byte_permit = Arc::clone(&self.bytes)
            .try_acquire_many_owned(size.total as u32)
            .map_err(|_| self.capacity_error())?;
        self.reserved_bytes.fetch_add(size.total, Ordering::Relaxed);
        self.active_jobs.fetch_add(1, Ordering::Relaxed);
        Ok(DeadLetterEncodingReservation {
            budget: Arc::clone(self),
            size,
            _byte_permit: byte_permit,
            _job_permit: job_permit,
        })
    }

    fn capacity_error(&self) -> DataLayerError {
        self.capacity_rejected_total.fetch_add(1, Ordering::Relaxed);
        DataLayerError::TimedOut("dead-letter encoding capacity is exhausted".to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EncodingSize {
    json: usize,
    total: usize,
}

fn encoding_size(entry: &RuntimeQueueEntry, error: &str) -> Option<EncodingSize> {
    let mut raw = entry.id.len().checked_add(error.len())?;
    for (key, value) in &entry.fields {
        raw = raw.checked_add(key.len())?.checked_add(value.len())?;
    }
    checked_encoding_size(raw, entry.fields.len())
}

fn checked_encoding_size(raw: usize, field_count: usize) -> Option<EncodingSize> {
    // Every string byte needs at most six bytes (\u00XX). Each map entry adds
    // four quotes, a colon and at most one comma to the empty envelope.
    const EMPTY_ENVELOPE_BYTES: usize = br#"{"entry_id":"","fields":{},"error":""}"#.len();
    let json = raw
        .checked_mul(6)?
        .checked_add(field_count.checked_mul(6)?)?
        .checked_add(EMPTY_ENVELOPE_BYTES)?;
    Some(EncodingSize {
        json,
        total: raw.checked_add(json)?,
    })
}

pub(crate) struct DeadLetterEncodingReservation {
    budget: Arc<DeadLetterEncodingBudget>,
    size: EncodingSize,
    _byte_permit: OwnedSemaphorePermit,
    _job_permit: OwnedSemaphorePermit,
}

impl DeadLetterEncodingReservation {
    pub(crate) fn encode_owned(
        self,
        entry: RuntimeQueueEntry,
        error: String,
    ) -> impl std::future::Future<Output = Result<EncodedDeadLetter, DataLayerError>> + Send {
        let input = EncodingInput {
            entry,
            error,
            reservation: self,
        };
        async move {
            tokio::task::spawn_blocking(move || input.encode())
                .await
                .map_err(|error| {
                    DataLayerError::UnexpectedValue(format!(
                        "dead-letter encoding task failed: {error}"
                    ))
                })?
        }
    }
}

impl Drop for DeadLetterEncodingReservation {
    fn drop(&mut self) {
        self.budget
            .reserved_bytes
            .fetch_sub(self.size.total, Ordering::Relaxed);
        self.budget.active_jobs.fetch_sub(1, Ordering::Relaxed);
    }
}

// Field order also protects cancellation/panic: raw data is dropped before its
// reservation, including when a queued blocking task never starts.
struct EncodingInput {
    entry: RuntimeQueueEntry,
    error: String,
    reservation: DeadLetterEncodingReservation,
}

#[derive(Serialize)]
struct DeadLetterPayload<'a> {
    entry_id: &'a str,
    fields: &'a BTreeMap<String, String>,
    error: &'a str,
}

impl EncodingInput {
    fn encode(self) -> Result<EncodedDeadLetter, DataLayerError> {
        #[cfg(test)]
        {
            let hook = self.reservation.budget.encode_hook.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
        }
        let mut writer = BoundedJsonWriter::new(self.reservation.size.json);
        serde_json::to_writer(
            &mut writer,
            &DeadLetterPayload {
                entry_id: &self.entry.id,
                fields: &self.entry.fields,
                error: &self.error,
            },
        )
        .map_err(|error| {
            DataLayerError::UnexpectedValue(format!(
                "failed to encode complete dead-letter fields: {error}"
            ))
        })?;
        let payload = String::from_utf8(writer.bytes).map_err(|error| {
            DataLayerError::UnexpectedValue(format!("dead-letter JSON was not UTF-8: {error}"))
        })?;
        self.reservation
            .budget
            .encoded_total
            .fetch_add(1, Ordering::Relaxed);
        Ok(EncodedDeadLetter {
            entry_id: self.entry.id,
            fields: BTreeMap::from([("payload".to_string(), payload)]),
            _reservation: self.reservation,
        })
    }
}

pub(crate) struct EncodedDeadLetter {
    pub(crate) entry_id: String,
    pub(crate) fields: BTreeMap<String, String>,
    // Remains owned by the result until transfer/append finishes.
    _reservation: DeadLetterEncodingReservation,
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl BoundedJsonWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
        }
    }
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.max_bytes.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("dead-letter JSON encoding bound exceeded"));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.max_bytes);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|error| {
                    io::Error::other(format!("dead-letter JSON allocation failed: {error}"))
                })?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn test_entry() -> RuntimeQueueEntry {
        RuntimeQueueEntry {
            id: "10-3".to_string(),
            fields: BTreeMap::from([
                ("payload".to_string(), "historical payload".to_string()),
                ("extra".to_string(), "original metadata".to_string()),
            ]),
        }
    }

    #[tokio::test]
    async fn dead_letter_encoding_preserves_complete_wire_and_all_escape_forms() {
        let control_bytes = (0u8..=31).map(char::from).collect::<String>();
        let mut entry = test_entry();
        entry.id.push_str("\"\\\n");
        entry.fields.insert(
            format!("{control_bytes}\"\\"),
            format!("{control_bytes}\"\\\u{4e2d}\u{6587}\u{1f600}"),
        );
        let error = format!("error:{control_bytes}\"\\\u{00e9}");
        let expected = serde_json::to_string(&DeadLetterPayload {
            entry_id: &entry.id,
            fields: &entry.fields,
            error: &error,
        })
        .unwrap();
        let size = encoding_size(&entry, &error).unwrap();
        assert!(expected.len() <= size.json);
        let budget = Arc::new(DeadLetterEncodingBudget::new(size.total, 1));
        let encoded = budget
            .try_reserve(&entry, &error)
            .unwrap()
            .encode_owned(entry.clone(), error.clone())
            .await
            .unwrap();
        assert_eq!(encoded.entry_id, entry.id);
        assert_eq!(encoded.fields.len(), 1);
        assert_eq!(encoded.fields["payload"], expected);
        let decoded: serde_json::Value = serde_json::from_str(&encoded.fields["payload"]).unwrap();
        assert_eq!(
            decoded["fields"],
            serde_json::to_value(entry.fields).unwrap()
        );
        assert_eq!(decoded["error"], error);
        assert_eq!(budget.snapshot().encoded_total, 1);
        assert_eq!(budget.snapshot().reserved_bytes, size.total);
        assert_eq!(budget.snapshot().active_jobs, 1);
        drop(encoded);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().active_jobs, 0);
    }

    #[test]
    fn dead_letter_encoding_budget_rejects_oversize_and_saturation_without_waiters() {
        let entry = test_entry();
        let size = encoding_size(&entry, "failure").unwrap();
        let too_small = Arc::new(DeadLetterEncodingBudget::new(size.total - 1, 1));
        assert!(matches!(
            too_small.try_reserve(&entry, "failure"),
            Err(DataLayerError::InvalidInput(_))
        ));
        assert_eq!(too_small.snapshot().oversized_rejected_total, 1);
        assert_eq!(too_small.snapshot().reserved_bytes, 0);
        assert_eq!(too_small.snapshot().active_jobs, 0);

        for (limit, jobs) in [(size.total, 2), (size.total * 2, 1)] {
            let budget = Arc::new(DeadLetterEncodingBudget::new(limit, jobs));
            let first = budget.try_reserve(&entry, "failure").unwrap();
            assert!(matches!(
                budget.try_reserve(&entry, "failure"),
                Err(DataLayerError::TimedOut(_))
            ));
            assert_eq!(budget.snapshot().capacity_rejected_total, 1);
            assert_eq!(budget.snapshot().reserved_bytes, size.total);
            assert_eq!(budget.snapshot().active_jobs, 1);
            assert_eq!(budget.jobs.available_permits(), jobs - 1);
            drop(first);
            assert_eq!(budget.snapshot().reserved_bytes, 0);
            assert_eq!(budget.jobs.available_permits(), jobs);
            assert_eq!(budget.bytes.available_permits(), limit);
            drop(budget.try_reserve(&entry, "failure").unwrap());
        }
    }

    #[test]
    fn dead_letter_encoding_bounds_and_environment_cannot_overflow() {
        assert!(checked_encoding_size(usize::MAX, 0).is_none());
        assert!(checked_encoding_size(usize::MAX / 6, 0).is_none());
        assert!(checked_encoding_size(0, usize::MAX).is_none());
        assert!(checked_encoding_size(usize::MAX / 7, 1).is_none());
        assert!(checked_encoding_size(0, 0).unwrap().total > 0);
        assert_eq!(configured_limit(None, 4, 128), 4);
        assert_eq!(configured_limit(Some("0"), 4, 128), 4);
        assert_eq!(configured_limit(Some("invalid"), 4, 128), 4);
        assert_eq!(configured_limit(Some(" 2 "), 4, 128), 2);
        assert_eq!(configured_limit(Some("99999999999"), 4, 128), 128);
        assert_eq!(
            configured_limit(Some(&u128::MAX.to_string()), 4, maximum_budget_bytes()),
            maximum_budget_bytes()
        );
        let budget = DeadLetterEncodingBudget::new(usize::MAX, usize::MAX);
        assert_eq!(budget.snapshot().limit_bytes, maximum_budget_bytes());
        assert_eq!(budget.snapshot().job_limit, MAX_ENCODING_JOBS);
    }

    #[test]
    fn dead_letter_encoding_bounded_writer_grows_geometrically_and_stops_at_limit() {
        let mut writer = BoundedJsonWriter::new(4096);
        let mut allocations = 0;
        for _ in 0..4096 {
            let previous_capacity = writer.bytes.capacity();
            writer.write_all(b"x").unwrap();
            allocations += usize::from(previous_capacity != writer.bytes.capacity());
        }
        assert!(allocations <= 13, "allocations: {allocations}");
        assert!(writer.write_all(b"y").is_err());
        assert_eq!(writer.bytes.len(), 4096);
        assert!(writer.bytes.iter().all(|byte| *byte == b'x'));
    }

    #[tokio::test]
    async fn dead_letter_encoding_unpolled_cancellation_releases_reservation() {
        let entry = test_entry();
        let budget = Arc::new(DeadLetterEncodingBudget::new(4096, 1));
        let reservation = budget.try_reserve(&entry, "failure").unwrap();
        let future = reservation.encode_owned(entry, "failure".to_string());
        assert_eq!(budget.snapshot().active_jobs, 1);
        drop(future);
        assert_eq!(budget.snapshot().active_jobs, 0);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().encoded_total, 0);
    }

    #[tokio::test]
    async fn dead_letter_encoding_running_cancellation_holds_budget_until_closure_exits() {
        let entry = test_entry();
        let budget = Arc::new(DeadLetterEncodingBudget::new(4096, 1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *budget.encode_hook.lock().unwrap() = Some(Box::new(move || {
            let _ = started_tx.send(());
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }));
        let reservation = budget.try_reserve(&entry, "failure").unwrap();
        let task = tokio::spawn(reservation.encode_owned(entry, "failure".to_string()));
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert_eq!(budget.snapshot().active_jobs, 1);
        assert!(budget.snapshot().reserved_bytes > 0);
        assert!(budget.try_reserve(&test_entry(), "failure").is_err());
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while budget.snapshot().active_jobs != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().encoded_total, 1);
    }

    #[tokio::test]
    async fn dead_letter_encoding_panic_releases_input_and_reservation() {
        let entry = test_entry();
        let budget = Arc::new(DeadLetterEncodingBudget::new(4096, 1));
        *budget.encode_hook.lock().unwrap() = Some(Box::new(|| panic!("encoding test panic")));
        let result = budget
            .try_reserve(&entry, "failure")
            .unwrap()
            .encode_owned(entry, "failure".to_string())
            .await;
        assert!(matches!(result, Err(DataLayerError::UnexpectedValue(_))));
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().active_jobs, 0);
        assert_eq!(budget.snapshot().encoded_total, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dead_letter_encoding_concurrent_reservations_have_no_hidden_waiting_queue() {
        const TASKS: usize = 16;
        let entry = Arc::new(test_entry());
        let size = encoding_size(&entry, "failure").unwrap();
        let budget = Arc::new(DeadLetterEncodingBudget::new(size.total * 2, 2));
        let ready = Arc::new(tokio::sync::Barrier::new(TASKS + 1));
        let release = Arc::new(tokio::sync::Barrier::new(TASKS + 1));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..TASKS {
            let entry = Arc::clone(&entry);
            let budget = Arc::clone(&budget);
            let ready = Arc::clone(&ready);
            let release = Arc::clone(&release);
            tasks.spawn(async move {
                let reservation = budget.try_reserve(&entry, "failure");
                ready.wait().await;
                release.wait().await;
                reservation.is_ok()
            });
        }
        tokio::time::timeout(Duration::from_secs(2), ready.wait())
            .await
            .unwrap();
        assert_eq!(budget.snapshot().active_jobs, 2);
        assert_eq!(budget.snapshot().reserved_bytes, size.total * 2);
        assert_eq!(
            budget.snapshot().capacity_rejected_total,
            (TASKS - 2) as u64
        );
        release.wait().await;
        let mut admitted = 0;
        while let Some(result) = tasks.join_next().await {
            admitted += usize::from(result.unwrap());
        }
        assert_eq!(admitted, 2);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().active_jobs, 0);
    }
}
