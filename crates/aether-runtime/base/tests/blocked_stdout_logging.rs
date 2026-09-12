use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aether_runtime::{
    init_service_runtime, logging_metric_samples, FileLoggingConfig, LogDestination, LogFormat,
    LogRotation, LogShutdownGuard, ServiceRuntimeConfig,
};

const CHILD_ENV: &str = "AETHER_TEST_BLOCKED_STDOUT_CHILD";
const DIRECTORY_ENV: &str = "AETHER_TEST_BLOCKED_STDOUT_DIR";
const SERVICE_NAME: &str = "blocked-stdout-test";
const EVENT_NAME: &str = "blocked_stdout_probe";
const TEST_NAME: &str = "blocked_stdout_does_not_block_file_logs_or_process_exit";

#[test]
fn blocked_stdout_does_not_block_file_logs_or_process_exit() {
    if std::env::var_os(CHILD_ENV).is_some() {
        run_child_scenario();
        eprintln!("blocked stdout guard returned");
        // The scenario returns normally and drops its guard. Skip libtest's own
        // stdout report, while still exercising Rust's standard exit cleanup.
        std::process::exit(0);
    }

    let directory = TestDirectory::new();
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args(["--exact", TEST_NAME, "--nocapture", "--quiet"])
        .env(CHILD_ENV, "1")
        .env(DIRECTORY_ENV, &directory.0)
        .env_remove("RUST_LOG")
        .env_remove("NO_COLOR")
        .env_remove("FORCE_COLOR");
    let mut child = BlockedStdoutChild::spawn(&mut command).expect("logging child should start");
    let (status, timed_out, stderr) = child
        .wait(Duration::from_secs(8))
        .expect("logging child should be reaped");
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        !timed_out,
        "blocked stdout prevented process exit within 8 seconds: {stderr}"
    );
    assert!(status.success(), "logging child failed: {status}: {stderr}");
    assert!(
        stderr.contains("blocked stdout saturated")
            && stderr.contains("blocked stdout guard returned"),
        "child did not reach saturation and return from its guard: {stderr}"
    );

    let records = read_file_records(&directory.0);
    assert!(
        !records.is_empty(),
        "healthy file destination received no logs"
    );
    let final_markers: Vec<_> = records
        .iter()
        .filter(|record| record["fields"]["phase"] == "final")
        .collect();
    assert_eq!(
        final_markers.len(),
        1,
        "file lost or duplicated final marker"
    );
    let final_marker = final_markers[0];
    assert_eq!(final_marker["fields"]["event_name"], EVENT_NAME);
    assert!(
        final_marker["fields"]["stdout_dropped_full"]
            .as_u64()
            .expect("stdout queue drop counter")
            + final_marker["fields"]["stdout_dropped_bytes"]
                .as_u64()
                .expect("stdout byte drop counter")
            > 0,
        "file marker must prove stdout saturation"
    );
    assert_eq!(records.last().unwrap()["fields"]["phase"], "final");
}

fn run_child_scenario() {
    let _shutdown = LogShutdownGuard::new();
    let directory = PathBuf::from(std::env::var_os(DIRECTORY_ENV).expect("child log directory"));
    init_service_runtime(
        ServiceRuntimeConfig::new(SERVICE_NAME, "info")
            .with_log_destination(LogDestination::Both)
            .with_log_format(LogFormat::Json)
            .with_file_logging(FileLoggingConfig::new(directory, LogRotation::Daily, 7, 30)),
    )
    .expect("both log destinations should initialize");

    let payload = "x".repeat(384);
    let flood_deadline = Instant::now() + Duration::from_secs(3);
    let mut emitted = 0;
    while emitted < 20_000 && Instant::now() < flood_deadline {
        tracing::info!(
            event_name = EVENT_NAME,
            phase = "flood",
            sequence = emitted,
            payload = %payload,
            "fill unread stdout"
        );
        emitted += 1;
        if emitted % 64 == 0 && stdout_dropped_events() > 0 {
            break;
        }
    }
    assert!(stdout_dropped_events() > 0, "stdout queue did not saturate");

    let file_deadline = Instant::now() + Duration::from_secs(1);
    while metric("logging_file_retained_bytes") > 0 && Instant::now() < file_deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        metric("logging_file_retained_bytes"),
        0,
        "file writer stalled"
    );
    assert_eq!(metric("logging_file_write_errors_total"), 0);
    let dropped_full = metric("logging_stdout_dropped_full_total");
    let dropped_bytes = metric("logging_stdout_dropped_bytes_total");
    eprintln!(
        "blocked stdout saturated: full={dropped_full} bytes={dropped_bytes} emitted={emitted}"
    );
    tracing::info!(
        event_name = EVENT_NAME,
        phase = "final",
        stdout_dropped_full = dropped_full,
        stdout_dropped_bytes = dropped_bytes,
        "healthy file final marker"
    );
}

fn metric(name: &str) -> u64 {
    logging_metric_samples()
        .into_iter()
        .find(|sample| sample.name == name)
        .unwrap_or_else(|| panic!("missing logging metric: {name}"))
        .value
}

fn stdout_dropped_events() -> u64 {
    metric("logging_stdout_dropped_full_total") + metric("logging_stdout_dropped_bytes_total")
}

fn read_file_records(directory: &Path) -> Vec<serde_json::Value> {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .expect("log directory")
        .map(|entry| entry.expect("log entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(SERVICE_NAME) && name.ends_with(".log"))
        })
        .collect();
    paths.sort();
    let mut records = Vec::new();
    for path in paths {
        let contents = fs::read_to_string(path).expect("UTF-8 file logs");
        assert!(contents.ends_with('\n'), "partial final file record");
        for line in contents.lines() {
            assert!(line.len() < 1024, "test event exceeded 1 KiB");
            records.push(serde_json::from_str(line).expect("complete JSON file record"));
        }
    }
    records
}

struct BlockedStdoutChild {
    child: Child,
    stderr_reader: Option<JoinHandle<io::Result<Vec<u8>>>>,
}

impl BlockedStdoutChild {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        let child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut guarded = Self {
            child,
            stderr_reader: None,
        };
        let mut stderr = guarded.child.stderr.take().expect("child stderr pipe");
        guarded.stderr_reader = Some(std::thread::Builder::new().spawn(move || {
            let mut captured = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let count = stderr.read(&mut chunk)?;
                if count == 0 {
                    return Ok(captured);
                }
                let retained = count.min((16 * 1024usize).saturating_sub(captured.len()));
                captured.extend_from_slice(&chunk[..retained]);
            }
        })?);
        Ok(guarded)
    }

    fn wait(&mut self, timeout: Duration) -> io::Result<(ExitStatus, bool, Vec<u8>)> {
        let deadline = Instant::now() + timeout;
        let (status, timed_out) = loop {
            if let Some(status) = self.child.try_wait()? {
                break (status, false);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                break (self.child.wait()?, true);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Keep the stdout read end open and completely unread until the child
        // has exited or been killed. Closing it earlier would unblock writes.
        drop(self.child.stdout.take());
        let stderr = self
            .stderr_reader
            .take()
            .expect("stderr reader")
            .join()
            .map_err(|_| io::Error::other("stderr reader panicked"))??;
        Ok((status, timed_out, stderr))
    }
}

impl Drop for BlockedStdoutChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop(self.child.stdout.take());
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("aether-blocked-stdout-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
