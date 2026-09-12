#![cfg(target_os = "linux")]

use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use aether_runtime::{
    init_reloadable_service_tracing, init_service_runtime, shutdown_logging, FileLoggingConfig,
    LogDestination, LogFormat, LogRotation, ServiceRuntimeConfig,
};

#[test]
#[ignore = "requires the fixture image from tests/root_logging.Dockerfile and the production capability profile"]
fn root_appends_to_existing_logs_without_changing_ownership() {
    if let Ok(scenario) = std::env::var("AETHER_TEST_ROOT_LOGGING_CASE") {
        run_scenario(&scenario);
        return;
    }

    for entrypoint in ["standard", "reloadable"] {
        for destination in ["file", "both"] {
            for format in ["pretty", "json"] {
                for owner in ["0", "1000", "65532", "new"] {
                    let scenario = format!("{entrypoint}-{destination}-{format}-{owner}");
                    let mut command =
                        Command::new(std::env::current_exe().expect("test executable"));
                    command
                        .args([
                            "--ignored",
                            "--exact",
                            "root_appends_to_existing_logs_without_changing_ownership",
                            "--nocapture",
                        ])
                        .env("AETHER_TEST_ROOT_LOGGING_CASE", &scenario)
                        .env_remove("RUST_LOG");
                    let output = run_subprocess(&mut command);
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    assert!(output.status.success(), "{scenario}: {stdout}\n{stderr}");
                    assert_eq!(
                        stdout.matches("root logging ready").count(),
                        usize::from(destination == "both"),
                        "{scenario}: {stdout}"
                    );
                    assert!(!stderr.contains("stdout logging"), "{scenario}: {stderr}");
                }
            }
        }
    }
}

fn run_subprocess(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("root logging subprocess");
    let mut stdout = child.stdout.take().expect("stdout pipe");
    let mut stderr = child.stderr.take().expect("stderr pipe");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("read child stdout");
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).expect("read child stderr");
        bytes
    });
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
        "root logging subprocess timed out: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn run_scenario(scenario: &str) {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    assert_eq!(unsafe { libc::getegid() }, 0);
    let process_status = fs::read_to_string("/proc/self/status").expect("process status");
    for capability in ["CapEff", "CapPrm", "CapBnd"] {
        let value = process_status
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{capability}:")))
            .expect("capability field");
        assert_eq!(
            u64::from_str_radix(value.trim(), 16).expect("capability bits"),
            0xa
        );
    }
    assert!(process_status.lines().any(|line| {
        line.strip_prefix("NoNewPrivs:")
            .is_some_and(|value| value.trim() == "1")
    }));

    let parts: Vec<_> = scenario.split('-').collect();
    let [entrypoint, destination, format, owner] = parts.as_slice() else {
        panic!("invalid scenario: {scenario}");
    };
    let dir = PathBuf::from("/logs").join(scenario);
    let bucket = chrono::Local::now().format("%Y-%m-%d");
    let log_file = dir.join(format!("root-logging-test.{bucket}.log"));
    let directory_metadata = fs::metadata(&dir).expect("pre-existing foreign-owned directory");
    assert_eq!(directory_metadata.uid(), 1000);
    assert_eq!(directory_metadata.gid(), 1000);
    assert_eq!(directory_metadata.permissions().mode() & 0o777, 0o750);

    let expected_owner = if *owner == "new" {
        assert!(!log_file.exists());
        0
    } else {
        let owner_uid: u32 = owner.parse().expect("fixture owner");
        let metadata = fs::metadata(&log_file).expect("pre-existing log");
        assert_eq!(metadata.uid(), owner_uid);
        assert_eq!(metadata.gid(), owner_uid);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
        owner_uid
    };

    let config = ServiceRuntimeConfig::new("root-logging-test", "info")
        .with_log_destination(match *destination {
            "file" => LogDestination::File,
            "both" => LogDestination::Both,
            other => panic!("unknown destination: {other}"),
        })
        .with_log_format(match *format {
            "pretty" => LogFormat::Pretty,
            "json" => LogFormat::Json,
            other => panic!("unknown format: {other}"),
        })
        .with_file_logging(FileLoggingConfig::new(&dir, LogRotation::Daily, 7, 30));

    let _reloader = match *entrypoint {
        "standard" => {
            init_service_runtime(config).expect("root should initialize file logging");
            None
        }
        "reloadable" => Some(
            init_reloadable_service_tracing("info", config)
                .expect("root should initialize reloadable file logging"),
        ),
        other => panic!("unknown entrypoint: {other}"),
    };
    tracing::info!("root logging ready");
    assert!(
        shutdown_logging(Duration::from_secs(2)),
        "root file logging should drain"
    );

    let metadata = fs::metadata(&log_file).expect("written log file");
    assert_eq!(metadata.uid(), expected_owner);
    assert_eq!(metadata.gid(), expected_owner);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    let contents = fs::read_to_string(&log_file).expect("log contents");
    assert_eq!(contents.matches("root logging ready").count(), 1);
    if *owner != "new" {
        assert!(contents.starts_with("historical log\n"));
    }
    if *format == "json" {
        let event: serde_json::Value =
            serde_json::from_str(contents.lines().last().expect("log event"))
                .expect("JSON file log");
        assert_eq!(event["fields"]["message"], "root logging ready");
    }
    let directory_metadata = fs::metadata(&dir).expect("log directory");
    assert_eq!(directory_metadata.uid(), 1000);
    assert_eq!(directory_metadata.gid(), 1000);
    assert_eq!(directory_metadata.permissions().mode() & 0o777, 0o750);
}
