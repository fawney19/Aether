use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing_subscriber::fmt::writer::MakeWriter;

use crate::metrics::{MetricKind, MetricSample};

const QUEUE_CAPACITY: usize = 4096;
const BYTE_LIMIT: usize = 8 * 1024 * 1024;
const EVENT_LIMIT: usize = 256 * 1024;
const LOCAL_WORKER_DROP_TIMEOUT: Duration = Duration::from_millis(100);

static LOG_WORKERS: OnceLock<Vec<LogWorker>> = OnceLock::new();

#[derive(Clone, Copy)]
struct Limits {
    queue_capacity: usize,
    bytes: usize,
    event_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            queue_capacity: QUEUE_CAPACITY,
            bytes: BYTE_LIMIT,
            event_bytes: EVENT_LIMIT,
        }
    }
}

struct State {
    destination: &'static str,
    limits: Limits,
    accepting: AtomicBool,
    running: AtomicBool,
    retained_bytes: AtomicUsize,
    retained_events: AtomicUsize,
    accepted: AtomicU64,
    dropped_full: AtomicU64,
    dropped_bytes: AtomicU64,
    dropped_oversize: AtomicU64,
    dropped_closed: AtomicU64,
    write_errors: AtomicU64,
    worker_panics: AtomicU64,
    shutdown_timeouts: AtomicU64,
}

impl State {
    fn new(destination: &'static str, limits: Limits) -> Self {
        Self {
            destination,
            limits,
            accepting: AtomicBool::new(true),
            running: AtomicBool::new(true),
            retained_bytes: AtomicUsize::new(0),
            retained_events: AtomicUsize::new(0),
            accepted: AtomicU64::new(0),
            dropped_full: AtomicU64::new(0),
            dropped_bytes: AtomicU64::new(0),
            dropped_oversize: AtomicU64::new(0),
            dropped_closed: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
            worker_panics: AtomicU64::new(0),
            shutdown_timeouts: AtomicU64::new(0),
        }
    }

    fn reserve(self: &Arc<Self>, bytes: usize) -> Option<RetainedBytes> {
        self.retained_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |retained| {
                retained
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limits.bytes)
            })
            .ok()?;
        self.retained_events.fetch_add(1, Ordering::AcqRel);
        Some(RetainedBytes {
            state: Arc::clone(self),
            bytes,
        })
    }
}

struct RetainedBytes {
    state: Arc<State>,
    bytes: usize,
}

impl Drop for RetainedBytes {
    fn drop(&mut self) {
        self.state
            .retained_bytes
            .fetch_sub(self.bytes, Ordering::AcqRel);
        self.state.retained_events.fetch_sub(1, Ordering::AcqRel);
    }
}

struct BufferedEvent {
    bytes: Box<[u8]>,
    // Field order frees the allocation before making its budget available again.
    _retained: RetainedBytes,
}

enum Message {
    Event(BufferedEvent),
    Wake,
}

#[derive(Default)]
struct Completion {
    result: Mutex<Option<bool>>,
    changed: Condvar,
}

impl Completion {
    fn finish(&self, success: bool) {
        *self
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(success);
        self.changed.notify_all();
    }

    fn wait(&self, timeout: Duration) -> Option<bool> {
        let result = self
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (result, _) = self
            .changed
            .wait_timeout_while(result, timeout, |result| result.is_none())
            .unwrap_or_else(|error| error.into_inner());
        *result
    }
}

struct CompletionGuard {
    state: Arc<State>,
    completion: Arc<Completion>,
    success: bool,
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.state.accepting.store(false, Ordering::Release);
        self.state.running.store(false, Ordering::Release);
        self.completion.finish(self.success);
    }
}

#[derive(Clone)]
pub(super) struct NonBlockingLogWriter {
    sender: mpsc::Sender<Message>,
    state: Arc<State>,
}

impl NonBlockingLogWriter {
    pub(super) fn new(
        destination: &'static str,
        sink: impl Write + Send + 'static,
    ) -> io::Result<(Self, LogWorker)> {
        Self::with_limits(destination, sink, Limits::default())
    }

    fn with_limits(
        destination: &'static str,
        sink: impl Write + Send + 'static,
        limits: Limits,
    ) -> io::Result<(Self, LogWorker)> {
        if limits.queue_capacity == 0 || limits.bytes == 0 || limits.event_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log writer limits must be positive",
            ));
        }
        let (sender, receiver) = mpsc::channel(limits.queue_capacity);
        let state = Arc::new(State::new(destination, limits));
        let completion = Arc::new(Completion::default());
        let worker_state = Arc::clone(&state);
        let worker_completion = Arc::clone(&completion);
        // The thread owns blocking I/O. No join is attempted when a sink stalls.
        std::thread::Builder::new()
            .name(format!("aether-log-{destination}"))
            .spawn(move || {
                let mut completed = CompletionGuard {
                    state: Arc::clone(&worker_state),
                    completion: worker_completion,
                    success: false,
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_worker(receiver, sink, &worker_state)
                }));
                completed.success = match result {
                    Ok(success) => success,
                    Err(_) => {
                        worker_state.worker_panics.fetch_add(1, Ordering::Relaxed);
                        false
                    }
                };
            })?;
        Ok((
            Self {
                sender: sender.clone(),
                state: Arc::clone(&state),
            },
            LogWorker {
                sender,
                state,
                completion,
            },
        ))
    }

    fn enqueue(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if !self.state.accepting.load(Ordering::Acquire) {
            self.state.dropped_closed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if bytes.len() > self.state.limits.event_bytes {
            self.state.dropped_oversize.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Some(retained) = self.state.reserve(bytes.len()) else {
            self.state.dropped_bytes.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if !self.state.accepting.load(Ordering::Acquire) {
            self.state.dropped_closed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let event = BufferedEvent {
            bytes: Box::from(bytes),
            _retained: retained,
        };
        self.enqueue_reserved(event);
    }

    fn enqueue_reserved(&self, event: BufferedEvent) {
        // Do not reserve channel slots: an unpublished reservation could prevent
        // the shutdown Wake from reaching an otherwise idle receiver.
        match self.sender.try_send(Message::Event(event)) {
            Ok(()) => {
                self.state.accepted.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.state.dropped_full.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.state.dropped_closed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Write for NonBlockingLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // The fmt layer supplies one complete formatted event to write_all.
        // A rejection must not cause retries or synchronous stderr diagnostics.
        self.enqueue(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // A producer flush cannot wait for a blocked destination. Shutdown owns it.
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for NonBlockingLogWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn run_worker(mut receiver: mpsc::Receiver<Message>, mut sink: impl Write, state: &State) -> bool {
    loop {
        if !state.accepting.load(Ordering::Acquire) {
            receiver.close();
        }
        match receiver.blocking_recv() {
            Some(Message::Event(event)) => {
                if sink.write_all(&event.bytes).is_err() {
                    state.write_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            Some(Message::Wake) => receiver.close(),
            None => break,
        }
    }
    match sink.flush() {
        Ok(()) => true,
        Err(_) => {
            state.write_errors.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

pub(super) struct LogWorker {
    sender: mpsc::Sender<Message>,
    state: Arc<State>,
    completion: Arc<Completion>,
}

impl LogWorker {
    fn close(&self) {
        self.state.accepting.store(false, Ordering::Release);
        // A full queue already wakes the worker, which checks accepting each turn.
        let _ = self.sender.try_send(Message::Wake);
    }

    fn wait(&self, timeout: Duration) -> bool {
        match self.completion.wait(timeout) {
            Some(success) => success,
            None => {
                self.state.shutdown_timeouts.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }
}

impl Drop for LogWorker {
    fn drop(&mut self) {
        self.close();
        self.wait(LOCAL_WORKER_DROP_TIMEOUT);
    }
}

pub(super) fn register_log_workers(workers: Vec<LogWorker>) {
    // Failed or duplicate initialization drops only its own local workers.
    let _ = LOG_WORKERS.set(workers);
}

fn shutdown_workers(workers: &[LogWorker], timeout: Duration) -> bool {
    let started = Instant::now();
    for worker in workers {
        worker.close();
    }
    let mut finished = true;
    for worker in workers {
        finished &= worker.wait(timeout.saturating_sub(started.elapsed()));
    }
    finished
}

/// Stops accepting logs and waits within one total deadline for drain and flush.
/// A true result reports completed workers and a successful final flush; previous
/// write failures may already have lost events and remain in write error metrics.
/// This does not promise fsync durability or cancel a destination's blocked I/O.
/// Producers already preparing an event may release their reservations afterward.
pub fn shutdown_logging(timeout: Duration) -> bool {
    LOG_WORKERS
        .get()
        .is_none_or(|workers| shutdown_workers(workers, timeout))
}

#[must_use = "keep the logging guard alive until service shutdown finishes"]
pub struct LogShutdownGuard;

impl LogShutdownGuard {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogShutdownGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LogShutdownGuard {
    fn drop(&mut self) {
        shutdown_logging(Duration::from_secs(2));
    }
}

#[derive(Default)]
struct Snapshot {
    workers: u64,
    byte_limit: u64,
    event_limit: u64,
    queue_capacity: u64,
    retained_bytes: u64,
    retained_events: u64,
    accepted: u64,
    dropped_full: u64,
    dropped_bytes: u64,
    dropped_oversize: u64,
    dropped_closed: u64,
    write_errors: u64,
    worker_panics: u64,
    shutdown_timeouts: u64,
    accepting: u64,
    running: u64,
}

impl Snapshot {
    fn add(&mut self, state: &State) {
        self.workers += 1;
        self.byte_limit += state.limits.bytes as u64;
        self.event_limit = self.event_limit.max(state.limits.event_bytes as u64);
        self.queue_capacity += state.limits.queue_capacity as u64;
        self.retained_bytes += state.retained_bytes.load(Ordering::Relaxed) as u64;
        self.retained_events += state.retained_events.load(Ordering::Relaxed) as u64;
        self.accepted += state.accepted.load(Ordering::Relaxed);
        self.dropped_full += state.dropped_full.load(Ordering::Relaxed);
        self.dropped_bytes += state.dropped_bytes.load(Ordering::Relaxed);
        self.dropped_oversize += state.dropped_oversize.load(Ordering::Relaxed);
        self.dropped_closed += state.dropped_closed.load(Ordering::Relaxed);
        self.write_errors += state.write_errors.load(Ordering::Relaxed);
        self.worker_panics += state.worker_panics.load(Ordering::Relaxed);
        self.shutdown_timeouts += state.shutdown_timeouts.load(Ordering::Relaxed);
        self.accepting += u64::from(state.accepting.load(Ordering::Relaxed));
        self.running += u64::from(state.running.load(Ordering::Relaxed));
    }
}

fn metric_samples(workers: &[LogWorker]) -> Vec<MetricSample> {
    let mut stdout = Snapshot::default();
    let mut file = Snapshot::default();
    let mut other = Snapshot::default();
    for worker in workers {
        match worker.state.destination {
            "stdout" => stdout.add(&worker.state),
            "file" => file.add(&worker.state),
            _ => other.add(&worker.state),
        }
    }
    let mut samples = Vec::new();
    macro_rules! append {
        ($prefix:literal, $snapshot:ident) => {
            if $snapshot.workers > 0 {
                macro_rules! metric {
                    ($field:ident, $suffix:literal, $help:literal, $kind:ident) => {
                        samples.push(MetricSample::new(
                            concat!($prefix, $suffix),
                            $help,
                            MetricKind::$kind,
                            $snapshot.$field,
                        ));
                    };
                }
                metric!(
                    byte_limit,
                    "_byte_limit",
                    "Log buffer byte limit, including writes in progress.",
                    Gauge
                );
                metric!(
                    event_limit,
                    "_event_byte_limit",
                    "Maximum bytes accepted in one formatted log event.",
                    Gauge
                );
                metric!(
                    queue_capacity,
                    "_queue_capacity",
                    "Maximum queued log events excluding the active write.",
                    Gauge
                );
                metric!(
                    retained_bytes,
                    "_retained_bytes",
                    "Owned log bytes including producer reservations and active writes.",
                    Gauge
                );
                metric!(
                    retained_events,
                    "_retained_events",
                    "Owned log events including producer reservations and active writes.",
                    Gauge
                );
                metric!(
                    accepted,
                    "_accepted_total",
                    "Log events accepted for background writing.",
                    Counter
                );
                metric!(
                    dropped_full,
                    "_dropped_full_total",
                    "Log events dropped because the queue was full.",
                    Counter
                );
                metric!(
                    dropped_bytes,
                    "_dropped_bytes_total",
                    "Log events dropped because the byte budget was full.",
                    Counter
                );
                metric!(
                    dropped_oversize,
                    "_dropped_oversize_total",
                    "Log events exceeding the per-event byte limit.",
                    Counter
                );
                metric!(
                    dropped_closed,
                    "_dropped_closed_total",
                    "Log events dropped because the writer was closing or closed.",
                    Counter
                );
                metric!(
                    write_errors,
                    "_write_errors_total",
                    "Background log write or flush errors.",
                    Counter
                );
                metric!(
                    worker_panics,
                    "_worker_panics_total",
                    "Background log worker panics.",
                    Counter
                );
                metric!(
                    shutdown_timeouts,
                    "_shutdown_timeouts_total",
                    "Log shutdown waits that exceeded their deadline.",
                    Counter
                );
                metric!(
                    accepting,
                    "_accepting_workers",
                    "Log workers accepting new events.",
                    Gauge
                );
                metric!(
                    running,
                    "_running_workers",
                    "Log workers not yet finished draining and flushing.",
                    Gauge
                );
            }
        };
    }
    append!("logging_stdout", stdout);
    append!("logging_file", file);
    append!("logging_other", other);
    samples
}

pub fn logging_metric_samples() -> Vec<MetricSample> {
    LOG_WORKERS
        .get()
        .map_or_else(Vec::new, |workers| metric_samples(workers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;
    use tracing_subscriber::prelude::*;

    use super::super::{
        JsonRuntimeEventFormatter, PrettyRuntimeEventFormatter, RuntimeLogIdentity,
    };

    #[derive(Clone, Default)]
    struct Buffer {
        events: Arc<Mutex<Vec<Vec<u8>>>>,
        flushes: Arc<AtomicUsize>,
    }

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.events.lock().unwrap().push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct Release(Arc<(Mutex<bool>, Condvar)>);

    impl Release {
        fn open(&self) {
            *self.0 .0.lock().unwrap() = true;
            self.0 .1.notify_all();
        }
    }

    impl Drop for Release {
        fn drop(&mut self) {
            self.open();
        }
    }

    struct BlockedSink {
        release: Arc<(Mutex<bool>, Condvar)>,
        entered: Option<std_mpsc::Sender<()>>,
        buffer: Buffer,
    }

    impl Write for BlockedSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                entered.send(()).unwrap();
            }
            let guard = self.release.0.lock().unwrap();
            drop(self.release.1.wait_while(guard, |open| !*open).unwrap());
            self.buffer.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.buffer.flush()
        }
    }

    fn blocked_sink() -> (BlockedSink, Release, std_mpsc::Receiver<()>, Buffer) {
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered, observed) = std_mpsc::channel();
        let buffer = Buffer::default();
        (
            BlockedSink {
                release: Arc::clone(&release),
                entered: Some(entered),
                buffer: buffer.clone(),
            },
            Release(release),
            observed,
            buffer,
        )
    }

    fn assert_released(state: &State) {
        assert_eq!(state.retained_bytes.load(Ordering::Acquire), 0);
        assert_eq!(state.retained_events.load(Ordering::Acquire), 0);
        assert!(!state.running.load(Ordering::Acquire));
    }

    #[test]
    fn nonblocking_log_writer_slow_sink_does_not_block_concurrent_producers() {
        let (sink, release, entered, buffer) = blocked_sink();
        let (mut writer, worker) = NonBlockingLogWriter::with_limits(
            "stdout",
            sink,
            Limits {
                queue_capacity: 4,
                bytes: 1024,
                event_bytes: 8,
            },
        )
        .unwrap();
        writer.write_all(b"first\n").unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let (finished, completed) = std_mpsc::channel();
        let threads = (0..8)
            .map(|_| {
                let mut writer = writer.clone();
                let finished = finished.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        writer.write_all(b"event\n").unwrap();
                    }
                    finished.send(()).unwrap();
                })
            })
            .collect::<Vec<_>>();
        for _ in 0..8 {
            completed
                .recv_timeout(Duration::from_secs(2))
                .expect("producers must finish while sink is blocked");
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(writer.state.retained_bytes.load(Ordering::Acquire), 30);
        assert_eq!(writer.state.accepted.load(Ordering::Relaxed), 5);
        assert_eq!(writer.state.dropped_full.load(Ordering::Relaxed), 796);
        release.open();
        assert!(shutdown_workers(&[worker], Duration::from_secs(2)));
        assert_eq!(buffer.events.lock().unwrap().len(), 5);
        assert_released(&writer.state);
    }

    #[test]
    fn nonblocking_log_writer_byte_limit_includes_active_write_and_rejects_whole_events() {
        let (sink, release, entered, buffer) = blocked_sink();
        let (mut writer, worker) = NonBlockingLogWriter::with_limits(
            "file",
            sink,
            Limits {
                queue_capacity: 8,
                bytes: 12,
                event_bytes: 8,
            },
        )
        .unwrap();
        writer.write_all(b"12345678").unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        writer.write_all(b"12345").unwrap();
        assert_eq!(writer.state.dropped_bytes.load(Ordering::Relaxed), 1);
        writer.write_all(b"123456789").unwrap();
        assert_eq!(writer.state.dropped_oversize.load(Ordering::Relaxed), 1);
        writer.write_all(b"1234").unwrap();
        assert_eq!(writer.state.retained_bytes.load(Ordering::Acquire), 12);
        release.open();
        assert!(shutdown_workers(&[worker], Duration::from_secs(2)));
        assert_eq!(
            *buffer.events.lock().unwrap(),
            [b"12345678".to_vec(), b"1234".to_vec()]
        );
        assert_released(&writer.state);
        writer.write_all(b"closed").unwrap();
        assert_eq!(writer.state.dropped_closed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn nonblocking_log_writer_shutdown_rejects_a_producer_still_preparing_an_event() {
        let buffer = Buffer::default();
        let (writer, worker) = NonBlockingLogWriter::new("stdout", buffer.clone()).unwrap();
        let retained = writer.state.reserve(6).unwrap();
        worker.close();
        assert!(worker.wait(Duration::from_secs(2)));
        assert_eq!(writer.state.retained_bytes.load(Ordering::Acquire), 6);
        writer.enqueue_reserved(BufferedEvent {
            bytes: Box::from(&b"raced\n"[..]),
            _retained: retained,
        });
        assert!(buffer.events.lock().unwrap().is_empty());
        assert_eq!(writer.state.accepted.load(Ordering::Relaxed), 0);
        assert_eq!(writer.state.dropped_closed.load(Ordering::Relaxed), 1);
        assert_released(&writer.state);
    }

    #[test]
    fn nonblocking_log_writer_concurrent_shutdown_drains_every_accepted_event() {
        let buffer = Buffer::default();
        let (mut writer, worker) = NonBlockingLogWriter::new("stdout", buffer.clone()).unwrap();
        writer.write_all(b"before\n").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(9));
        let producers = (0..8)
            .map(|_| {
                let mut writer = writer.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..128 {
                        writer.write_all(b"concurrent\n").unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        worker.close();
        for producer in producers {
            producer.join().unwrap();
        }
        assert!(worker.wait(Duration::from_secs(2)));
        let accepted = writer.state.accepted.load(Ordering::Relaxed);
        let dropped = writer.state.dropped_closed.load(Ordering::Relaxed);
        assert_eq!(accepted + dropped, 1 + 8 * 128);
        assert_eq!(buffer.events.lock().unwrap().len() as u64, accepted);
        assert_released(&writer.state);
    }

    #[test]
    fn nonblocking_log_writer_real_formatters_preserve_concurrent_event_boundaries() {
        for json in [false, true] {
            let buffer = Buffer::default();
            let (writer, worker) = NonBlockingLogWriter::new("stdout", buffer.clone()).unwrap();
            let identity = RuntimeLogIdentity {
                service: "concurrent-log-test",
                node_role: Some("gateway".to_owned()),
                instance_id: Some("test-1".to_owned()),
            };
            let dispatch = if json {
                tracing::Dispatch::new(
                    tracing_subscriber::registry().with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_writer(writer.clone())
                            .event_format(JsonRuntimeEventFormatter::new(identity)),
                    ),
                )
            } else {
                tracing::Dispatch::new(
                    tracing_subscriber::registry().with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(writer.clone())
                            .event_format(PrettyRuntimeEventFormatter::new(identity, false)),
                    ),
                )
            };
            let producers = (0..8_u64)
                .map(|producer| {
                    let dispatch = dispatch.clone();
                    std::thread::spawn(move || {
                        tracing::dispatcher::with_default(&dispatch, || {
                            for sequence in 0..64_u64 {
                                let event_id = producer * 64 + sequence;
                                tracing::info!(
                                    target: "concurrent_log_test",
                                    event_id,
                                    enabled = true,
                                    ratio = 1.5_f64,
                                    note = "first\nsecond",
                                    "event-{event_id:03} \"quoted\" \\"
                                );
                            }
                        });
                    })
                })
                .collect::<Vec<_>>();
            for producer in producers {
                producer.join().unwrap();
            }
            assert!(shutdown_workers(&[worker], Duration::from_secs(2)));
            let events = buffer.events.lock().unwrap();
            assert_eq!(events.len(), 512, "each write must contain one whole event");
            let mut observed = std::collections::BTreeSet::new();
            for event in events.iter() {
                let text = std::str::from_utf8(event).unwrap();
                assert_eq!(text.lines().count(), 1, "no partial or merged records");
                let event_id = if json {
                    let payload: serde_json::Value = serde_json::from_str(text).unwrap();
                    assert_eq!(payload["service"], "concurrent-log-test");
                    assert_eq!(payload["node_role"], "gateway");
                    assert_eq!(payload["instance_id"], "test-1");
                    assert_eq!(payload["fields"]["enabled"], true);
                    assert_eq!(payload["fields"]["ratio"], 1.5);
                    assert_eq!(payload["fields"]["note"], "first\nsecond");
                    let event_id = payload["fields"]["event_id"].as_u64().unwrap();
                    assert_eq!(
                        payload["fields"]["message"],
                        format!("event-{event_id:03} \"quoted\" \\")
                    );
                    event_id
                } else {
                    assert!(!text.contains('\u{1b}'));
                    assert!(text.contains("enabled=true ratio=1.5 note=\"first\\nsecond\""));
                    let event_id = text
                        .split_once("event_id=")
                        .unwrap()
                        .1
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap();
                    assert!(text.contains(&format!(" - event-{event_id:03} \"quoted\" \\")));
                    event_id
                };
                assert!(observed.insert(event_id), "duplicate event {event_id}");
            }
            assert_eq!(observed, (0..512).collect());
            assert_eq!(writer.state.accepted.load(Ordering::Relaxed), 512);
            assert_released(&writer.state);
        }
    }

    #[test]
    fn nonblocking_log_writer_shutdown_closes_all_sinks_before_waiting() {
        for (slow_destination, healthy_destination) in [("stdout", "file"), ("file", "stdout")] {
            let (sink, release, entered, _) = blocked_sink();
            let (mut slow, slow_worker) =
                NonBlockingLogWriter::new(slow_destination, sink).unwrap();
            let buffer = Buffer::default();
            let (mut healthy, healthy_worker) =
                NonBlockingLogWriter::new(healthy_destination, buffer.clone()).unwrap();
            slow.write_all(b"stalled\n").unwrap();
            entered.recv_timeout(Duration::from_secs(2)).unwrap();
            healthy.write_all(b"healthy\n").unwrap();
            let workers = [slow_worker, healthy_worker];
            assert!(!shutdown_workers(&workers, Duration::from_millis(50)));
            assert!(!healthy.state.accepting.load(Ordering::Acquire));
            assert!(workers[1].wait(Duration::from_secs(2)));
            assert_eq!(*buffer.events.lock().unwrap(), [b"healthy\n".to_vec()]);
            assert_eq!(buffer.flushes.load(Ordering::SeqCst), 1);
            assert_eq!(slow.state.shutdown_timeouts.load(Ordering::Relaxed), 1);
            release.open();
            assert!(shutdown_workers(&workers, Duration::from_secs(2)));
            assert_released(&slow.state);
            assert_released(&healthy.state);
        }
    }

    #[test]
    fn nonblocking_log_writer_blocked_flush_respects_deadline_and_other_sink_drains() {
        struct BlockedFlushSink {
            release: Arc<(Mutex<bool>, Condvar)>,
            entered: std_mpsc::Sender<()>,
            buffer: Buffer,
        }

        impl Write for BlockedFlushSink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.buffer.write(bytes)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.entered.send(()).unwrap();
                let guard = self.release.0.lock().unwrap();
                drop(self.release.1.wait_while(guard, |open| !*open).unwrap());
                self.buffer.flush()
            }
        }

        let release = Release(Arc::new((Mutex::new(false), Condvar::new())));
        let (entered, observed) = std_mpsc::channel();
        let flushed_buffer = Buffer::default();
        let (mut flushing, flushing_worker) = NonBlockingLogWriter::new(
            "file",
            BlockedFlushSink {
                release: Arc::clone(&release.0),
                entered,
                buffer: flushed_buffer.clone(),
            },
        )
        .unwrap();
        let healthy_buffer = Buffer::default();
        let (mut healthy, healthy_worker) =
            NonBlockingLogWriter::new("stdout", healthy_buffer.clone()).unwrap();
        flushing.write_all(b"before flush\n").unwrap();
        flushing_worker.close();
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(flushed_buffer.flushes.load(Ordering::SeqCst), 0);
        assert_eq!(flushing.state.retained_bytes.load(Ordering::Acquire), 0);
        healthy.write_all(b"while other sink flushes\n").unwrap();
        let (finished, completion) = std_mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            let workers = [flushing_worker, healthy_worker];
            let success = shutdown_workers(&workers, Duration::from_millis(50));
            assert!(finished.send((success, workers)).is_ok());
        });
        let (success, workers) = completion
            .recv_timeout(Duration::from_secs(2))
            .expect("shutdown must return while the destination is still flushing");
        shutdown.join().unwrap();
        assert!(!success);
        assert!(workers[1].wait(Duration::from_secs(2)));
        assert_eq!(
            *healthy_buffer.events.lock().unwrap(),
            [b"while other sink flushes\n".to_vec()]
        );
        assert_eq!(healthy_buffer.flushes.load(Ordering::SeqCst), 1);
        assert_eq!(flushing.state.shutdown_timeouts.load(Ordering::Relaxed), 1);
        assert!(flushing.state.running.load(Ordering::Acquire));
        release.open();
        assert!(shutdown_workers(&workers, Duration::from_secs(2)));
        assert_eq!(flushed_buffer.flushes.load(Ordering::SeqCst), 1);
        assert_released(&flushing.state);
        assert_released(&healthy.state);
    }

    struct FailingSink {
        fail_flush: bool,
    }

    impl Write for FailingSink {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::Error::other("flush failed"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn nonblocking_log_writer_io_errors_do_not_block_producers_or_leak_budget() {
        for fail_flush in [false, true] {
            let (mut writer, worker) =
                NonBlockingLogWriter::new("file", FailingSink { fail_flush }).unwrap();
            writer.write_all(b"one\n").unwrap();
            writer.write_all(b"two\n").unwrap();
            assert_eq!(
                shutdown_workers(&[worker], Duration::from_secs(2)),
                !fail_flush
            );
            assert_eq!(
                writer.state.write_errors.load(Ordering::Relaxed),
                2 + u64::from(fail_flush)
            );
            assert_released(&writer.state);
        }
    }

    #[test]
    fn nonblocking_log_writer_panic_releases_pending_budget_and_marks_worker_finished() {
        struct PanicSink;
        impl Write for PanicSink {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                panic!("sink panic")
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (mut writer, worker) = NonBlockingLogWriter::new("file", PanicSink).unwrap();
        for _ in 0..8 {
            writer.write_all(b"event\n").unwrap();
        }
        assert!(!shutdown_workers(&[worker], Duration::from_secs(2)));
        assert_eq!(writer.state.worker_panics.load(Ordering::Relaxed), 1);
        assert_released(&writer.state);
    }

    #[test]
    fn nonblocking_log_writer_failed_initialization_drop_wakes_and_flushes_idle_worker() {
        let buffer = Buffer::default();
        let (writer, worker) = NonBlockingLogWriter::new("stdout", buffer.clone()).unwrap();
        let completion = Arc::clone(&worker.completion);
        drop(worker);
        assert_eq!(completion.wait(Duration::from_secs(2)), Some(true));
        assert_eq!(buffer.flushes.load(Ordering::SeqCst), 1);
        assert_released(&writer.state);
    }

    #[test]
    fn nonblocking_log_writer_metrics_use_unique_destination_names() {
        let (_, stdout) = NonBlockingLogWriter::new("stdout", io::sink()).unwrap();
        let (_, file) = NonBlockingLogWriter::new("file", io::sink()).unwrap();
        let workers = [stdout, file];
        let samples = metric_samples(&workers);
        let names = samples
            .iter()
            .map(|sample| sample.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), samples.len());
        assert!(samples
            .iter()
            .any(|sample| sample.name == "logging_stdout_byte_limit"
                && sample.value == BYTE_LIMIT as u64));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "logging_file_byte_limit"
                && sample.value == BYTE_LIMIT as u64));
        assert!(shutdown_workers(&workers, Duration::from_secs(2)));
    }
}
