// Runtime sampling is shared by standalone load tools and integration tests.
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sysinfo::{get_current_pid, Pid, ProcessesToUpdate, System};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct BenchmarkRuntimeSnapshot {
    pub sampled_at_unix_secs: u64,
    pub elapsed_ms: u64,
    pub system_cpu_usage_basis_points: u64,
    pub process_cpu_usage_basis_points: u64,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub memory_available_bytes: u64,
    pub memory_used_basis_points: u64,
    pub process_memory_bytes: u64,
    pub process_virtual_memory_bytes: u64,
    pub process_memory_basis_points: u64,
    pub fd_open_count: u64,
    pub fd_limit: u64,
}

pub struct BenchmarkRuntimeSampler {
    started_at: Instant,
    system: System,
    current_pid: Option<Pid>,
}

impl BenchmarkRuntimeSampler {
    pub fn new() -> Self {
        let mut system = System::new_all();
        let current_pid = get_current_pid().ok();
        if let Some(pid) = current_pid {
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        }
        system.refresh_cpu_usage();
        system.refresh_memory();
        Self {
            started_at: Instant::now(),
            system,
            current_pid,
        }
    }

    pub fn snapshot(&mut self) -> BenchmarkRuntimeSnapshot {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        if let Some(pid) = self.current_pid {
            self.system
                .refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        }

        let memory_total_bytes = self.system.total_memory();
        let memory_used_bytes = self.system.used_memory();
        let memory_available_bytes = self.system.available_memory();
        let (process_cpu_usage_basis_points, process_memory_bytes, process_virtual_memory_bytes) =
            self.current_pid
                .and_then(|pid| self.system.process(pid))
                .map(|process| {
                    (
                        percent_to_basis_points(process.cpu_usage() as f64),
                        process.memory(),
                        process.virtual_memory(),
                    )
                })
                .unwrap_or((0, 0, 0));

        BenchmarkRuntimeSnapshot {
            sampled_at_unix_secs: current_unix_secs(),
            elapsed_ms: self.started_at.elapsed().as_millis() as u64,
            system_cpu_usage_basis_points: percent_to_basis_points(
                self.system.global_cpu_usage() as f64
            ),
            process_cpu_usage_basis_points,
            memory_total_bytes,
            memory_used_bytes,
            memory_available_bytes,
            memory_used_basis_points: ratio_to_basis_points(memory_used_bytes, memory_total_bytes),
            process_memory_bytes,
            process_virtual_memory_bytes,
            process_memory_basis_points: ratio_to_basis_points(
                process_memory_bytes,
                memory_total_bytes,
            ),
            fd_open_count: open_file_descriptors().unwrap_or(0),
            fd_limit: file_descriptor_limit(),
        }
    }
}

impl Default for BenchmarkRuntimeSampler {
    fn default() -> Self {
        Self::new()
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn percent_to_basis_points(value: f64) -> u64 {
    if !value.is_finite() || value.is_sign_negative() {
        0
    } else {
        (value * 100.0).round().clamp(0.0, u64::MAX as f64) as u64
    }
}

fn ratio_to_basis_points(value: u64, total: u64) -> u64 {
    value.saturating_mul(10_000).checked_div(total).unwrap_or(0)
}

fn open_file_descriptors() -> Option<u64> {
    #[cfg(unix)]
    {
        for dir in ["/proc/self/fd", "/dev/fd"] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                return Some(entries.count() as u64);
            }
        }
    }
    None
}

fn file_descriptor_limit() -> u64 {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let result = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        if result == 0 {
            return limit.rlim_cur;
        }
    }
    0
}
