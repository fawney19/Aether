use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_runtime::{MetricKind, MetricSample};
use sysinfo::{get_current_pid, Networks, Pid, ProcessesToUpdate, System};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GatewayProcessResourceSnapshot {
    pub(crate) sampled_at_unix_secs: u64,
    pub(crate) system_cpu_usage_basis_points: u64,
    pub(crate) process_cpu_usage_basis_points: u64,
    pub(crate) memory_total_bytes: u64,
    pub(crate) memory_used_bytes: u64,
    pub(crate) memory_available_bytes: u64,
    pub(crate) memory_used_basis_points: u64,
    pub(crate) process_memory_bytes: u64,
    pub(crate) process_virtual_memory_bytes: u64,
    pub(crate) process_memory_basis_points: u64,
    pub(crate) process_uptime_secs: Option<u64>,
    pub(crate) process_threads: u64,
    pub(crate) fd_open_count: u64,
    pub(crate) fd_limit: u64,
    pub(crate) fd_usage_basis_points: u64,
    pub(crate) network_observability_available: u64,
    pub(crate) network_interface_count: u64,
    pub(crate) network_received_bytes_total: u64,
    pub(crate) network_transmitted_bytes_total: u64,
    pub(crate) network_received_packets_total: u64,
    pub(crate) network_transmitted_packets_total: u64,
    pub(crate) network_receive_errors_total: u64,
    pub(crate) network_transmit_errors_total: u64,
    pub(crate) network_receive_dropped_total: u64,
    pub(crate) network_transmit_dropped_total: u64,
    pub(crate) process_socket_fds: u64,
    pub(crate) tcp_state_observability_available: u64,
    pub(crate) host_tcp_connections: u64,
    pub(crate) host_tcp_established_connections: u64,
    pub(crate) host_tcp_listen_connections: u64,
    pub(crate) host_tcp_time_wait_connections: u64,
    pub(crate) host_tcp_syn_sent_connections: u64,
    pub(crate) host_tcp_syn_recv_connections: u64,
    pub(crate) host_tcp_close_wait_connections: u64,
    pub(crate) process_tcp_connections: u64,
    pub(crate) process_tcp_established_connections: u64,
    pub(crate) process_tcp_listen_connections: u64,
    pub(crate) process_tcp_time_wait_connections: u64,
    pub(crate) process_tcp_syn_sent_connections: u64,
    pub(crate) process_tcp_syn_recv_connections: u64,
    pub(crate) process_tcp_close_wait_connections: u64,
}

impl GatewayProcessResourceSnapshot {
    pub(crate) fn to_metric_samples(self) -> Vec<MetricSample> {
        let mut samples = vec![
            MetricSample::new(
                "gateway_process_sampled_at_unix_secs",
                "Unix timestamp of the current gateway process resource sample.",
                MetricKind::Gauge,
                self.sampled_at_unix_secs,
            ),
            MetricSample::new(
                "gateway_system_cpu_usage_basis_points",
                "Host CPU usage in basis points of percent, where 10000 means 100 percent.",
                MetricKind::Gauge,
                self.system_cpu_usage_basis_points,
            ),
            MetricSample::new(
                "gateway_process_cpu_usage_basis_points",
                "Gateway process CPU usage in basis points of percent, where 10000 means 100 percent.",
                MetricKind::Gauge,
                self.process_cpu_usage_basis_points,
            ),
            MetricSample::new(
                "gateway_system_memory_total_bytes",
                "Total host memory visible to the gateway process.",
                MetricKind::Gauge,
                self.memory_total_bytes,
            ),
            MetricSample::new(
                "gateway_system_memory_used_bytes",
                "Used host memory visible to the gateway process.",
                MetricKind::Gauge,
                self.memory_used_bytes,
            ),
            MetricSample::new(
                "gateway_system_memory_available_bytes",
                "Available host memory visible to the gateway process.",
                MetricKind::Gauge,
                self.memory_available_bytes,
            ),
            MetricSample::new(
                "gateway_system_memory_usage_basis_points",
                "Host memory usage in basis points, where 10000 means 100 percent.",
                MetricKind::Gauge,
                self.memory_used_basis_points,
            ),
            MetricSample::new(
                "gateway_process_memory_bytes",
                "Gateway process resident memory bytes.",
                MetricKind::Gauge,
                self.process_memory_bytes,
            ),
            MetricSample::new(
                "gateway_process_virtual_memory_bytes",
                "Gateway process virtual memory bytes.",
                MetricKind::Gauge,
                self.process_virtual_memory_bytes,
            ),
            MetricSample::new(
                "gateway_process_memory_basis_points",
                "Gateway process resident memory as basis points of host memory.",
                MetricKind::Gauge,
                self.process_memory_basis_points,
            ),
            MetricSample::new(
                "gateway_process_threads",
                "Current number of threads owned by the gateway process where available.",
                MetricKind::Gauge,
                self.process_threads,
            ),
            MetricSample::new(
                "gateway_process_open_fds",
                "Current number of file descriptors opened by the gateway process.",
                MetricKind::Gauge,
                self.fd_open_count,
            ),
            MetricSample::new(
                "gateway_process_fd_limit",
                "Current soft file descriptor limit for the gateway process.",
                MetricKind::Gauge,
                self.fd_limit,
            ),
            MetricSample::new(
                "gateway_process_fd_usage_basis_points",
                "Gateway process file descriptor usage in basis points, where 10000 means 100 percent.",
                MetricKind::Gauge,
                self.fd_usage_basis_points,
            ),
            MetricSample::new(
                "gateway_network_observability_available",
                "Whether host network interface counters are available.",
                MetricKind::Gauge,
                self.network_observability_available,
            ),
            MetricSample::new(
                "gateway_network_interfaces",
                "Number of host network interfaces visible to the gateway process.",
                MetricKind::Gauge,
                self.network_interface_count,
            ),
            MetricSample::new(
                "gateway_network_received_bytes_total",
                "Total host network bytes received across visible interfaces.",
                MetricKind::Counter,
                self.network_received_bytes_total,
            ),
            MetricSample::new(
                "gateway_network_transmitted_bytes_total",
                "Total host network bytes transmitted across visible interfaces.",
                MetricKind::Counter,
                self.network_transmitted_bytes_total,
            ),
            MetricSample::new(
                "gateway_network_received_packets_total",
                "Total host network packets received across visible interfaces.",
                MetricKind::Counter,
                self.network_received_packets_total,
            ),
            MetricSample::new(
                "gateway_network_transmitted_packets_total",
                "Total host network packets transmitted across visible interfaces.",
                MetricKind::Counter,
                self.network_transmitted_packets_total,
            ),
            MetricSample::new(
                "gateway_network_receive_errors_total",
                "Total host network receive errors across visible interfaces.",
                MetricKind::Counter,
                self.network_receive_errors_total,
            ),
            MetricSample::new(
                "gateway_network_transmit_errors_total",
                "Total host network transmit errors across visible interfaces.",
                MetricKind::Counter,
                self.network_transmit_errors_total,
            ),
            MetricSample::new(
                "gateway_network_receive_dropped_total",
                "Total host network receive drops across visible interfaces where available.",
                MetricKind::Counter,
                self.network_receive_dropped_total,
            ),
            MetricSample::new(
                "gateway_network_transmit_dropped_total",
                "Total host network transmit drops across visible interfaces where available.",
                MetricKind::Counter,
                self.network_transmit_dropped_total,
            ),
            MetricSample::new(
                "gateway_process_socket_fds",
                "Current number of socket file descriptors opened by the gateway process.",
                MetricKind::Gauge,
                self.process_socket_fds,
            ),
            MetricSample::new(
                "gateway_tcp_state_observability_available",
                "Whether Linux TCP state counters are available from procfs.",
                MetricKind::Gauge,
                self.tcp_state_observability_available,
            ),
            MetricSample::new(
                "gateway_host_tcp_connections",
                "Current host TCP connections visible in procfs.",
                MetricKind::Gauge,
                self.host_tcp_connections,
            ),
            MetricSample::new(
                "gateway_host_tcp_established_connections",
                "Current host TCP connections in ESTABLISHED state visible in procfs.",
                MetricKind::Gauge,
                self.host_tcp_established_connections,
            ),
            MetricSample::new(
                "gateway_host_tcp_listen_connections",
                "Current host TCP sockets in LISTEN state visible in procfs.",
                MetricKind::Gauge,
                self.host_tcp_listen_connections,
            ),
            MetricSample::new(
                "gateway_host_tcp_time_wait_connections",
                "Current host TCP connections in TIME_WAIT state visible in procfs.",
                MetricKind::Gauge,
                self.host_tcp_time_wait_connections,
            ),
            MetricSample::new(
                "gateway_host_tcp_syn_sent_connections",
                "Current host TCP connections in SYN_SENT state visible in procfs.",
                MetricKind::Gauge,
                self.host_tcp_syn_sent_connections,
            ),
            MetricSample::new(
                "gateway_host_tcp_syn_recv_connections",
                "Current host TCP connections in SYN_RECV state visible in procfs.",
                MetricKind::Gauge,
                self.host_tcp_syn_recv_connections,
            ),
            MetricSample::new(
                "gateway_host_tcp_close_wait_connections",
                "Current host TCP connections in CLOSE_WAIT state visible in procfs.",
                MetricKind::Gauge,
                self.host_tcp_close_wait_connections,
            ),
            MetricSample::new(
                "gateway_process_tcp_connections",
                "Current gateway process TCP connections visible in procfs.",
                MetricKind::Gauge,
                self.process_tcp_connections,
            ),
            MetricSample::new(
                "gateway_process_tcp_established_connections",
                "Current gateway process TCP connections in ESTABLISHED state visible in procfs.",
                MetricKind::Gauge,
                self.process_tcp_established_connections,
            ),
            MetricSample::new(
                "gateway_process_tcp_listen_connections",
                "Current gateway process TCP sockets in LISTEN state visible in procfs.",
                MetricKind::Gauge,
                self.process_tcp_listen_connections,
            ),
            MetricSample::new(
                "gateway_process_tcp_time_wait_connections",
                "Current gateway process TCP connections in TIME_WAIT state visible in procfs.",
                MetricKind::Gauge,
                self.process_tcp_time_wait_connections,
            ),
            MetricSample::new(
                "gateway_process_tcp_syn_sent_connections",
                "Current gateway process TCP connections in SYN_SENT state visible in procfs.",
                MetricKind::Gauge,
                self.process_tcp_syn_sent_connections,
            ),
            MetricSample::new(
                "gateway_process_tcp_syn_recv_connections",
                "Current gateway process TCP connections in SYN_RECV state visible in procfs.",
                MetricKind::Gauge,
                self.process_tcp_syn_recv_connections,
            ),
            MetricSample::new(
                "gateway_process_tcp_close_wait_connections",
                "Current gateway process TCP connections in CLOSE_WAIT state visible in procfs.",
                MetricKind::Gauge,
                self.process_tcp_close_wait_connections,
            ),
        ];

        if let Some(process_uptime_secs) = self.process_uptime_secs {
            samples.push(MetricSample::new(
                "gateway_process_uptime_seconds",
                "Gateway process uptime in seconds.",
                MetricKind::Gauge,
                process_uptime_secs,
            ));
        }

        samples
    }
}

pub(crate) struct GatewayProcessResourceMonitor {
    system: Mutex<System>,
    networks: Mutex<Networks>,
    current_pid: Option<Pid>,
}

impl GatewayProcessResourceMonitor {
    pub(crate) fn new() -> Self {
        let mut system = System::new_all();
        let mut networks = Networks::new_with_refreshed_list();
        let current_pid = get_current_pid().ok();
        if let Some(pid) = current_pid {
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        }
        system.refresh_cpu_usage();
        system.refresh_memory();
        networks.refresh();
        Self {
            system: Mutex::new(system),
            networks: Mutex::new(networks),
            current_pid,
        }
    }

    pub(crate) fn snapshot(&self) -> GatewayProcessResourceSnapshot {
        let mut system = match self.system.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        system.refresh_cpu_usage();
        system.refresh_memory();
        if let Some(pid) = self.current_pid {
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        }

        let memory_total_bytes = system.total_memory();
        let memory_used_bytes = system.used_memory();
        let memory_available_bytes = system.available_memory();
        let (
            process_cpu_usage_basis_points,
            process_memory_bytes,
            process_virtual_memory_bytes,
            process_uptime_secs,
        ) = self
            .current_pid
            .and_then(|pid| system.process(pid))
            .map(|process| {
                (
                    percent_to_basis_points(process.cpu_usage() as f64),
                    process.memory(),
                    process.virtual_memory(),
                    Some(process.run_time()),
                )
            })
            .unwrap_or((0, 0, 0, None));
        let fd_open_count = open_file_descriptors().unwrap_or(0);
        let fd_limit = file_descriptor_limit();
        let network = self.network_snapshot();
        let sockets = socket_snapshot().unwrap_or_default();

        GatewayProcessResourceSnapshot {
            sampled_at_unix_secs: current_unix_secs(),
            system_cpu_usage_basis_points: percent_to_basis_points(system.global_cpu_usage() as f64),
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
            process_uptime_secs,
            process_threads: process_thread_count().unwrap_or(0),
            fd_open_count,
            fd_limit,
            fd_usage_basis_points: ratio_to_basis_points(fd_open_count, fd_limit),
            network_observability_available: network.observability_available,
            network_interface_count: network.interface_count,
            network_received_bytes_total: network.received_bytes_total,
            network_transmitted_bytes_total: network.transmitted_bytes_total,
            network_received_packets_total: network.received_packets_total,
            network_transmitted_packets_total: network.transmitted_packets_total,
            network_receive_errors_total: network.receive_errors_total,
            network_transmit_errors_total: network.transmit_errors_total,
            network_receive_dropped_total: network.receive_dropped_total,
            network_transmit_dropped_total: network.transmit_dropped_total,
            process_socket_fds: sockets.process_socket_fds,
            tcp_state_observability_available: sockets.tcp_state_observability_available,
            host_tcp_connections: sockets.host_tcp.total,
            host_tcp_established_connections: sockets.host_tcp.established,
            host_tcp_listen_connections: sockets.host_tcp.listen,
            host_tcp_time_wait_connections: sockets.host_tcp.time_wait,
            host_tcp_syn_sent_connections: sockets.host_tcp.syn_sent,
            host_tcp_syn_recv_connections: sockets.host_tcp.syn_recv,
            host_tcp_close_wait_connections: sockets.host_tcp.close_wait,
            process_tcp_connections: sockets.process_tcp.total,
            process_tcp_established_connections: sockets.process_tcp.established,
            process_tcp_listen_connections: sockets.process_tcp.listen,
            process_tcp_time_wait_connections: sockets.process_tcp.time_wait,
            process_tcp_syn_sent_connections: sockets.process_tcp.syn_sent,
            process_tcp_syn_recv_connections: sockets.process_tcp.syn_recv,
            process_tcp_close_wait_connections: sockets.process_tcp.close_wait,
        }
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        self.snapshot().to_metric_samples()
    }

    fn network_snapshot(&self) -> GatewayNetworkSnapshot {
        let mut networks = match self.networks.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        networks.refresh();

        let mut snapshot = GatewayNetworkSnapshot {
            observability_available: u64::from(!networks.list().is_empty()),
            interface_count: networks.list().len() as u64,
            ..GatewayNetworkSnapshot::default()
        };
        for network in networks.list().values() {
            snapshot.received_bytes_total = snapshot
                .received_bytes_total
                .saturating_add(network.total_received());
            snapshot.transmitted_bytes_total = snapshot
                .transmitted_bytes_total
                .saturating_add(network.total_transmitted());
            snapshot.received_packets_total = snapshot
                .received_packets_total
                .saturating_add(network.total_packets_received());
            snapshot.transmitted_packets_total = snapshot
                .transmitted_packets_total
                .saturating_add(network.total_packets_transmitted());
            snapshot.receive_errors_total = snapshot
                .receive_errors_total
                .saturating_add(network.total_errors_on_received());
            snapshot.transmit_errors_total = snapshot
                .transmit_errors_total
                .saturating_add(network.total_errors_on_transmitted());
        }

        let drops = network_drop_totals();
        snapshot.receive_dropped_total = drops.receive_dropped_total;
        snapshot.transmit_dropped_total = drops.transmit_dropped_total;
        snapshot
    }
}

impl Default for GatewayProcessResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for GatewayProcessResourceMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayProcessResourceMonitor")
            .field("current_pid", &self.current_pid)
            .finish_non_exhaustive()
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

fn process_thread_count() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|raw| parse_linux_process_thread_count(&raw));
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "linux")]
fn parse_linux_process_thread_count(raw: &str) -> Option<u64> {
    raw.lines().find_map(|line| {
        let value = line.strip_prefix("Threads:")?.trim();
        value.parse::<u64>().ok()
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct GatewayNetworkSnapshot {
    observability_available: u64,
    interface_count: u64,
    received_bytes_total: u64,
    transmitted_bytes_total: u64,
    received_packets_total: u64,
    transmitted_packets_total: u64,
    receive_errors_total: u64,
    transmit_errors_total: u64,
    receive_dropped_total: u64,
    transmit_dropped_total: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct NetworkDropTotals {
    receive_dropped_total: u64,
    transmit_dropped_total: u64,
}

fn network_drop_totals() -> NetworkDropTotals {
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/proc/net/dev")
            .ok()
            .map(|raw| parse_linux_network_drop_totals(&raw))
            .unwrap_or_default();
    }

    #[allow(unreachable_code)]
    NetworkDropTotals::default()
}

fn parse_linux_network_drop_totals(raw: &str) -> NetworkDropTotals {
    let mut totals = NetworkDropTotals::default();
    for line in raw.lines().skip(2) {
        let Some((_, counters)) = line.split_once(':') else {
            continue;
        };
        let fields: Vec<&str> = counters.split_whitespace().collect();
        if fields.len() < 12 {
            continue;
        }
        totals.receive_dropped_total = totals
            .receive_dropped_total
            .saturating_add(fields[3].parse::<u64>().unwrap_or_default());
        totals.transmit_dropped_total = totals
            .transmit_dropped_total
            .saturating_add(fields[11].parse::<u64>().unwrap_or_default());
    }
    totals
}

#[derive(Debug, Clone, Copy, Default)]
struct SocketSnapshot {
    process_socket_fds: u64,
    tcp_state_observability_available: u64,
    host_tcp: TcpStateCounts,
    process_tcp: TcpStateCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TcpStateCounts {
    total: u64,
    established: u64,
    listen: u64,
    time_wait: u64,
    syn_sent: u64,
    syn_recv: u64,
    close_wait: u64,
}

impl TcpStateCounts {
    fn observe(&mut self, state: &str) {
        self.total = self.total.saturating_add(1);
        match state {
            "01" => self.established = self.established.saturating_add(1),
            "02" => self.syn_sent = self.syn_sent.saturating_add(1),
            "03" => self.syn_recv = self.syn_recv.saturating_add(1),
            "06" => self.time_wait = self.time_wait.saturating_add(1),
            "08" => self.close_wait = self.close_wait.saturating_add(1),
            "0A" | "0a" => self.listen = self.listen.saturating_add(1),
            _ => {}
        }
    }
}

fn socket_snapshot() -> Option<SocketSnapshot> {
    #[cfg(target_os = "linux")]
    {
        let process_inodes = process_socket_inodes().unwrap_or_default();
        let mut snapshot = SocketSnapshot {
            process_socket_fds: process_inodes.len() as u64,
            ..SocketSnapshot::default()
        };
        let mut observed_tcp_table = false;

        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(raw) = std::fs::read_to_string(path) else {
                continue;
            };
            observed_tcp_table = true;
            observe_linux_tcp_table(&raw, &process_inodes, &mut snapshot);
        }

        if observed_tcp_table {
            snapshot.tcp_state_observability_available = 1;
            return Some(snapshot);
        }
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "linux")]
fn process_socket_inodes() -> Option<HashSet<u64>> {
    let mut inodes = HashSet::new();
    for entry in std::fs::read_dir("/proc/self/fd").ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        if let Some(inode) = parse_socket_inode(&target.to_string_lossy()) {
            inodes.insert(inode);
        }
    }
    Some(inodes)
}

fn parse_socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<u64>().ok())
}

fn observe_linux_tcp_table(
    raw: &str,
    process_inodes: &HashSet<u64>,
    snapshot: &mut SocketSnapshot,
) {
    for line in raw.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() <= 9 {
            continue;
        }
        let state = fields[3];
        snapshot.host_tcp.observe(state);
        let inode = fields[9].parse::<u64>().unwrap_or_default();
        if process_inodes.contains(&inode) {
            snapshot.process_tcp.observe(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_socket_inode_targets() {
        assert_eq!(parse_socket_inode("socket:[12345]"), Some(12345));
        assert_eq!(parse_socket_inode("anon_inode:[eventpoll]"), None);
        assert_eq!(parse_socket_inode("socket:12345"), None);
    }

    #[test]
    fn parses_linux_tcp_table_state_counts() {
        let raw = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000   501        0 111 1 0000000000000000 100 0 0 10 0
   1: 0100007F:9C40 0100007F:1F90 01 00000000:00000000 00:00000000 00000000   501        0 222 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:9C41 0100007F:1F90 08 00000000:00000000 00:00000000 00000000   501        0 333 1 0000000000000000 20 4 30 10 -1
";
        let mut inodes = HashSet::new();
        inodes.insert(111);
        inodes.insert(333);
        let mut snapshot = SocketSnapshot::default();

        observe_linux_tcp_table(raw, &inodes, &mut snapshot);

        assert_eq!(snapshot.host_tcp.total, 3);
        assert_eq!(snapshot.host_tcp.listen, 1);
        assert_eq!(snapshot.host_tcp.established, 1);
        assert_eq!(snapshot.host_tcp.close_wait, 1);
        assert_eq!(snapshot.process_tcp.total, 2);
        assert_eq!(snapshot.process_tcp.listen, 1);
        assert_eq!(snapshot.process_tcp.close_wait, 1);
    }

    #[test]
    fn parses_linux_network_drop_totals() {
        let raw = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 100 1 0 2 0 0 0 0 200 2 0 3 0 0 0 0
  eth0: 300 3 0 5 0 0 0 0 400 4 0 7 0 0 0 0
";

        let totals = parse_linux_network_drop_totals(raw);

        assert_eq!(totals.receive_dropped_total, 7);
        assert_eq!(totals.transmit_dropped_total, 10);
    }

    #[test]
    fn process_resource_monitor_renders_gateway_metrics() {
        let samples = GatewayProcessResourceMonitor::new().metric_samples();
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_process_memory_bytes"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_process_open_fds"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_process_fd_usage_basis_points"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_process_threads"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_process_socket_fds"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_network_observability_available"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_linux_process_thread_count() {
        let raw = "\
Name:\taether-gateway
State:\tS (sleeping)
Threads:\t42
";

        assert_eq!(parse_linux_process_thread_count(raw), Some(42));
        assert_eq!(parse_linux_process_thread_count("Name:\ttest\n"), None);
    }
}
