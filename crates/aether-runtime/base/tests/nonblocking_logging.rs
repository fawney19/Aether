use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use aether_runtime::{
    init_reloadable_service_tracing, init_service_runtime, FileLoggingConfig, LogDestination,
    LogFormat, LogRotation, LogShutdownGuard, ServiceRuntimeConfig,
};

const CASE_ENV: &str = "AETHER_TEST_NONBLOCKING_LOGGING_CASE";
const DIRECTORY_ENV: &str = "AETHER_TEST_NONBLOCKING_LOGGING_DIR";
const EVENT_NAME: &str = "nonblocking_logging_probe";
const SERVICE_NAME: &str = "nonblocking-logging-test";
const RECORD_COUNT: usize = 32;
const FIELD_VALUE: &str = "quote\" newline\n backslash\\ \u{4e2d}\u{6587}";

#[test]
fn nonblocking_logging_entrypoints_reload_and_guard_drain() {
    if let Ok(scenario) = std::env::var(CASE_ENV) {
        run_scenario(&scenario);
        return;
    }

    let root = TestDirectory::new();
    for entrypoint in ["standard", "reloadable"] {
        for destination in ["stdout", "file", "both"] {
            for format in ["pretty", "json"] {
                let scenario = format!("{entrypoint}-{destination}-{format}");
                let directory = root.0.join(&scenario);
                fs::create_dir(&directory).expect("scenario directory");
                let mut command = Command::new(std::env::current_exe().expect("test executable"));
                command
                    .args([
                        "--exact",
                        "nonblocking_logging_entrypoints_reload_and_guard_drain",
                        "--nocapture",
                        "--quiet",
                    ])
                    .env(CASE_ENV, &scenario)
                    .env(DIRECTORY_ENV, &directory)
                    .env_remove("RUST_LOG")
                    .env_remove("NO_COLOR")
                    .env_remove("FORCE_COLOR");
                let output = run_subprocess(&mut command);
                let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
                let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
                assert!(output.status.success(), "{scenario}: {stdout}\n{stderr}");
                let file_output = read_log_files(&directory);
                verify_output(
                    &stdout,
                    entrypoint,
                    format,
                    destination != "file",
                    &scenario,
                );
                verify_output(
                    &file_output,
                    entrypoint,
                    format,
                    destination != "stdout",
                    &scenario,
                );
            }
        }
    }
}

fn run_scenario(scenario: &str) {
    let parts: Vec<_> = scenario.split('-').collect();
    let [entrypoint, destination, format] = parts.as_slice() else {
        panic!("invalid logging scenario: {scenario}");
    };
    let _shutdown = LogShutdownGuard::new();
    let destination = match *destination {
        "stdout" => LogDestination::Stdout,
        "file" => LogDestination::File,
        "both" => LogDestination::Both,
        other => panic!("unknown destination: {other}"),
    };
    let mut config = ServiceRuntimeConfig::new(SERVICE_NAME, "info")
        .with_node_role("integration")
        .with_instance_id("logging-child")
        .with_log_destination(destination)
        .with_log_format(match *format {
            "pretty" => LogFormat::Pretty,
            "json" => LogFormat::Json,
            other => panic!("unknown format: {other}"),
        });
    if matches!(destination, LogDestination::File | LogDestination::Both) {
        config = config.with_file_logging(FileLoggingConfig::new(
            PathBuf::from(std::env::var_os(DIRECTORY_ENV).expect("scenario log directory")),
            LogRotation::Daily,
            7,
            30,
        ));
    }
    let reload = match *entrypoint {
        "standard" => {
            init_service_runtime(config).expect("standard logging initializes");
            None
        }
        "reloadable" => Some(
            init_reloadable_service_tracing("info", config)
                .expect("reloadable logging initializes"),
        ),
        other => panic!("unknown entrypoint: {other}"),
    };

    tracing::debug!(
        event_name = EVENT_NAME,
        phase = "initial_hidden",
        "filtered debug"
    );
    tracing::info!(event_name = EVENT_NAME, phase = "initial", "initial event");
    if let Some(reload) = reload {
        reload("debug");
        tracing::debug!(
            event_name = EVENT_NAME,
            phase = "reloaded_debug",
            "visible debug"
        );
        let invalid_filter = "nonblocking_logging=not-a-level";
        assert!(tracing_subscriber::EnvFilter::try_new(invalid_filter).is_err());
        reload(invalid_filter);
        tracing::debug!(
            event_name = EVENT_NAME,
            phase = "invalid_reload_unchanged",
            "still debug"
        );
        reload("error");
        tracing::info!(
            event_name = EVENT_NAME,
            phase = "error_filter_hidden",
            "filtered info"
        );
        tracing::error!(
            event_name = EVENT_NAME,
            phase = "reloaded_error",
            "visible error"
        );
        reload("info");
    }
    for sequence in 0..RECORD_COUNT {
        tracing::info!(
            event_name = EVENT_NAME,
            phase = "record",
            sequence = sequence as u64,
            value = FIELD_VALUE,
            "complete record"
        );
    }
    tracing::info!(
        event_name = EVENT_NAME,
        phase = "tail",
        "final event before guard drop"
    );
    // Returning drops the guard. The parent verifies the tail after process exit.
}

fn verify_output(output: &str, entrypoint: &str, format: &str, enabled: bool, scenario: &str) {
    let lines: Vec<_> = output
        .lines()
        .filter(|line| line.contains(EVENT_NAME))
        .collect();
    if !enabled {
        assert!(
            lines.is_empty(),
            "unexpected destination output in {scenario}: {output}"
        );
        return;
    }
    let expected_count = RECORD_COUNT + 2 + usize::from(entrypoint == "reloadable") * 3;
    assert_eq!(
        lines.len(),
        expected_count,
        "missing or duplicate records in {scenario}: {output}"
    );
    assert!(
        !output.contains('\u{1b}'),
        "redirected/file output must not contain ANSI: {scenario}"
    );
    assert!(
        !output.contains("initial_hidden"),
        "initial filter failed: {scenario}"
    );
    assert!(
        !output.contains("error_filter_hidden"),
        "reloaded filter failed: {scenario}"
    );
    let mut phases = Vec::new();
    let mut sequences = Vec::new();
    for line in lines {
        if format == "json" {
            let record: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("incomplete JSON in {scenario}: {error}: {line}"));
            assert_eq!(record["service"], SERVICE_NAME);
            assert_eq!(record["node_role"], "integration");
            assert_eq!(record["instance_id"], "logging-child");
            let phase = record["fields"]["phase"].as_str().expect("event phase");
            phases.push(phase.to_string());
            if phase == "record" {
                assert_eq!(record["fields"]["value"], FIELD_VALUE);
                sequences.push(
                    record["fields"]["sequence"]
                        .as_u64()
                        .expect("record sequence"),
                );
            }
        } else {
            assert!(
                line.contains(" | INFO") || line.contains(" | DEBUG") || line.contains(" | ERROR"),
                "incomplete Pretty record in {scenario}: {line}"
            );
            let phase = [
                "initial",
                "reloaded_debug",
                "invalid_reload_unchanged",
                "reloaded_error",
                "record",
                "tail",
            ]
            .into_iter()
            .find(|phase| line.contains(&format!("phase=\"{phase}\"")))
            .expect("complete Pretty phase field");
            phases.push(phase.to_string());
            if phase == "record" {
                let expected_value = format!("value={FIELD_VALUE:?}");
                assert!(
                    line.contains(&expected_value),
                    "incomplete Pretty value in {scenario}: {line}"
                );
                let sequence = line
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("sequence="))
                    .expect("complete Pretty sequence field")
                    .parse::<u64>()
                    .expect("sequence number");
                sequences.push(sequence);
            }
        }
    }
    assert_eq!(phases.first().map(String::as_str), Some("initial"));
    assert_eq!(
        phases.last().map(String::as_str),
        Some("tail"),
        "guard lost tail event: {scenario}"
    );
    for phase in [
        "reloaded_debug",
        "invalid_reload_unchanged",
        "reloaded_error",
    ] {
        assert_eq!(
            phases
                .iter()
                .filter(|value| value.as_str() == phase)
                .count(),
            usize::from(entrypoint == "reloadable"),
            "reload phase {phase} in {scenario}"
        );
    }
    assert_eq!(
        sequences,
        (0..RECORD_COUNT as u64).collect::<Vec<_>>(),
        "records must remain complete and ordered in {scenario}"
    );
}

fn read_log_files(directory: &Path) -> String {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .expect("log directory")
        .map(|entry| entry.expect("log directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(SERVICE_NAME) && name.ends_with(".log"))
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| fs::read_to_string(path).expect("UTF-8 file log"))
        .collect()
}

fn run_subprocess(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("logging subprocess");
    let stdout = child.stdout.take().expect("stdout pipe");
    let stderr = child.stderr.take().expect("stderr pipe");
    let stdout_reader = std::thread::spawn(move || read_pipe(stdout));
    let stderr_reader = std::thread::spawn(move || read_pipe(stderr));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().expect("child status") {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break child.wait().expect("reap timed out child");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = Output {
        status,
        stdout: stdout_reader.join().expect("stdout reader"),
        stderr: stderr_reader.join().expect("stderr reader"),
    };
    assert!(
        !timed_out,
        "logging subprocess timed out: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn read_pipe(mut pipe: impl Read) -> Vec<u8> {
    const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
    let mut captured = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = pipe.read(&mut chunk).expect("drain child pipe");
        if count == 0 {
            return captured;
        }
        let retained = count.min(MAX_CAPTURE_BYTES.saturating_sub(captured.len()));
        captured.extend_from_slice(&chunk[..retained]);
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("aether-nonblocking-logs-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
