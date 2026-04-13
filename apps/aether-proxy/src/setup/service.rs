//! Service installation and management for `aether-proxy`.
//!
//! Supports the host-native service manager we currently target:
//! `systemd` on most Linux distributions and `OpenRC` on Alpine.

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

const SERVICE_NAME: &str = "aether-proxy";

const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/aether-proxy.service";

const OPENRC_INIT_PATH: &str = "/etc/init.d/aether-proxy";
const OPENRC_PID_PATH: &str = "/run/aether-proxy.pid";
const OPENRC_LOG_DIR: &str = "/var/log/aether-proxy";
const OPENRC_STDOUT_LOG: &str = "/var/log/aether-proxy/current.log";
const OPENRC_STDERR_LOG: &str = "/var/log/aether-proxy/error.log";

const OPENRC_RUN_BINS: &[&str] = &["/sbin/openrc-run", "/usr/sbin/openrc-run", "openrc-run"];
const OPENRC_SERVICE_BINS: &[&str] = &["/sbin/rc-service", "/usr/sbin/rc-service", "rc-service"];
const OPENRC_UPDATE_BINS: &[&str] = &["/sbin/rc-update", "/usr/sbin/rc-update", "rc-update"];
const OPENRC_SUPERVISE_BINS: &[&str] = &[
    "/sbin/supervise-daemon",
    "/usr/sbin/supervise-daemon",
    "supervise-daemon",
];
const TAIL_BINS: &[&str] = &["/usr/bin/tail", "/bin/tail", "tail"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceManager {
    Systemd,
    OpenRc,
}

impl ServiceManager {
    fn display_name(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::OpenRc => "OpenRC",
        }
    }

    fn unit_path(self) -> &'static str {
        match self {
            Self::Systemd => SYSTEMD_UNIT_PATH,
            Self::OpenRc => OPENRC_INIT_PATH,
        }
    }

    fn is_installed(self) -> bool {
        Path::new(self.unit_path()).exists()
    }
}

pub fn is_available() -> bool {
    detect_service_manager().is_some() && is_root()
}

pub fn preferred_manager_name() -> &'static str {
    installed_manager()
        .or_else(detect_service_manager)
        .map(ServiceManager::display_name)
        .unwrap_or("service")
}

pub fn unavailable_hint() -> String {
    match detect_service_manager() {
        Some(manager) if !is_root() => {
            format!(
                "requires root with {}, use: sudo aether-proxy setup",
                manager.display_name()
            )
        }
        Some(manager) => format!(
            "{} is available but service setup is not ready",
            manager.display_name()
        ),
        None => "no supported service manager detected (systemd/OpenRC)".into(),
    }
}

pub fn install_service(config_path: &Path) -> anyhow::Result<()> {
    let manager = detect_service_manager()
        .ok_or_else(|| anyhow::anyhow!("no supported service manager detected (systemd/OpenRC)"))?;

    if !is_root() {
        anyhow::bail!("root required, use: sudo ./aether-proxy setup");
    }

    match manager {
        ServiceManager::Systemd => install_systemd_service(config_path),
        ServiceManager::OpenRc => install_openrc_service(config_path),
    }
}

pub(crate) fn is_root() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

pub fn is_installed() -> bool {
    installed_manager().is_some()
}

pub fn is_service_active() -> bool {
    active_service_manager().is_some()
}

pub fn restart_active_service() -> anyhow::Result<()> {
    let manager =
        active_service_manager().ok_or_else(|| anyhow::anyhow!("no active service detected"))?;
    restart_manager(manager)
}

pub fn uninstall_service() -> anyhow::Result<()> {
    let Some(manager) = installed_manager() else {
        return Ok(());
    };

    match manager {
        ServiceManager::Systemd => uninstall_systemd_service(),
        ServiceManager::OpenRc => uninstall_openrc_service(),
    }
}

pub fn cmd_status() -> anyhow::Result<()> {
    let manager = ensure_service_installed()?;
    let status = manager_status(manager)?;
    std::process::exit(status.code().unwrap_or(1));
}

pub fn cmd_logs() -> anyhow::Result<()> {
    let manager = ensure_service_installed()?;
    if manager == ServiceManager::OpenRc {
        ensure_openrc_logs_readable()?;
    }
    let status = match manager {
        ServiceManager::Systemd => Command::new("journalctl")
            .args(["-u", SERVICE_NAME, "-f", "--no-pager", "-n", "100"])
            .status()?,
        ServiceManager::OpenRc => Command::new(tail_bin())
            .args(["-n", "100", "-f", OPENRC_STDOUT_LOG, OPENRC_STDERR_LOG])
            .status()?,
    };
    std::process::exit(status.code().unwrap_or(1));
}

pub fn cmd_start() -> anyhow::Result<()> {
    let manager = ensure_root_and_service()?;
    start_manager(manager)?;
    eprintln!("  Service started.");
    Ok(())
}

pub fn cmd_restart() -> anyhow::Result<()> {
    let manager = ensure_root_and_service()?;
    restart_manager(manager)?;
    eprintln!("  Service restarted.");
    Ok(())
}

pub fn cmd_stop() -> anyhow::Result<()> {
    let manager = ensure_root_and_service()?;
    stop_manager(manager)?;
    eprintln!("  Service stopped.");
    Ok(())
}

pub fn cmd_uninstall() -> anyhow::Result<()> {
    ensure_root_and_service()?;
    uninstall_service()?;
    eprintln!();
    eprintln!("  Config file, TLS certs, and logs are preserved. Remove manually if needed.");
    Ok(())
}

pub(crate) fn run_cmd(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let display = format!("{} {}", program, args.join(" "));
    eprintln!("  > {}", display);

    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        anyhow::bail!("command failed: {}", display);
    }
    Ok(())
}

fn detect_service_manager() -> Option<ServiceManager> {
    if is_systemd_available() {
        Some(ServiceManager::Systemd)
    } else if is_openrc_available() {
        Some(ServiceManager::OpenRc)
    } else {
        None
    }
}

fn installed_manager() -> Option<ServiceManager> {
    if let Some(manager) = detect_service_manager() {
        if manager.is_installed() {
            return Some(manager);
        }
    }

    [ServiceManager::Systemd, ServiceManager::OpenRc]
        .into_iter()
        .find(|manager| manager.is_installed())
}

fn ensure_openrc_logs_readable() -> anyhow::Result<()> {
    for path in [OPENRC_STDOUT_LOG, OPENRC_STDERR_LOG] {
        match std::fs::File::open(path) {
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                anyhow::bail!(
                    "OpenRC logs are stored under {} and usually require root access. Try `sudo ./aether-proxy logs`.",
                    OPENRC_LOG_DIR
                );
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                anyhow::bail!(
                    "OpenRC log file not found at {}. Start the service first or check `./aether-proxy status`.",
                    path
                );
            }
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

fn active_service_manager() -> Option<ServiceManager> {
    if let Some(manager) = installed_manager() {
        if manager_is_active(manager) {
            return Some(manager);
        }
    }

    [ServiceManager::Systemd, ServiceManager::OpenRc]
        .into_iter()
        .find(|manager| manager_is_active(*manager))
}

fn ensure_service_installed() -> anyhow::Result<ServiceManager> {
    installed_manager().ok_or_else(|| {
        anyhow::anyhow!("service not installed, run `sudo ./aether-proxy setup` first")
    })
}

fn ensure_root_and_service() -> anyhow::Result<ServiceManager> {
    let manager = ensure_service_installed()?;
    if !is_root() {
        anyhow::bail!("root required, use: sudo ./aether-proxy <command>");
    }
    Ok(manager)
}

fn install_systemd_service(config_path: &Path) -> anyhow::Result<()> {
    let exe_path = std::env::current_exe()?.canonicalize()?;
    let exe_str = exe_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("binary path contains invalid UTF-8"))?;

    let config_abs = std::fs::canonicalize(config_path)?;
    let config_str = config_abs
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("config path contains invalid UTF-8"))?;

    let working_dir = config_abs
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_str()
        .unwrap_or("/");

    if Path::new(SYSTEMD_UNIT_PATH).exists() {
        eprintln!("  Stopping existing service...");
        let _ = Command::new("systemctl")
            .args(["stop", SERVICE_NAME])
            .status();
    }

    eprintln!("  Generating systemd unit file...");
    eprintln!("    Binary:  {}", exe_str);
    eprintln!("    Config:  {}", config_str);
    eprintln!("    WorkDir: {}", working_dir);

    let unit_content = format!(
        "[Unit]\n\
         Description=Aether Proxy\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={working_dir}\n\
         Environment=AETHER_PROXY_CONFIG={config_str}\n\
         Environment=AETHER_PROXY_SERVICE_MANAGER=systemd\n\
         Environment=AETHER_PROXY_LOG_DESTINATION=both\n\
         Environment=AETHER_PROXY_LOG_DIR=/var/log/aether-proxy\n\
         ExecStart={exe_str}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         LimitNOFILE=65535\n\
         UMask=0077\n\
         LogsDirectory=aether-proxy\n\
         LogsDirectoryMode=0750\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
    );
    std::fs::write(SYSTEMD_UNIT_PATH, &unit_content)?;

    eprintln!("  Enabling and starting service...");
    run_cmd("systemctl", &["daemon-reload"])?;
    run_cmd("systemctl", &["enable", "--now", SERVICE_NAME])?;

    eprintln!();
    if manager_is_active(ServiceManager::Systemd) {
        eprintln!("  Service started successfully!");
    } else {
        eprintln!("  Service state is not active yet. Check `sudo ./aether-proxy logs`.");
    }

    print_post_install_commands();
    Ok(())
}

fn install_openrc_service(config_path: &Path) -> anyhow::Result<()> {
    let exe_path = std::env::current_exe()?.canonicalize()?;
    let exe_str = exe_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("binary path contains invalid UTF-8"))?;

    let config_abs = std::fs::canonicalize(config_path)?;
    let config_str = config_abs
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("config path contains invalid UTF-8"))?;

    let working_dir = config_abs
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_str()
        .unwrap_or("/");

    if Path::new(OPENRC_INIT_PATH).exists() {
        eprintln!("  Stopping existing service...");
        let _ = Command::new(openrc_service_bin())
            .args([SERVICE_NAME, "stop"])
            .status();
    }

    std::fs::create_dir_all(OPENRC_LOG_DIR)?;
    touch_log(OPENRC_STDOUT_LOG)?;
    touch_log(OPENRC_STDERR_LOG)?;
    set_mode(OPENRC_LOG_DIR, 0o750)?;
    set_mode(OPENRC_STDOUT_LOG, 0o640)?;
    set_mode(OPENRC_STDERR_LOG, 0o640)?;

    eprintln!("  Generating OpenRC init script...");
    eprintln!("    Binary:  {}", exe_str);
    eprintln!("    Config:  {}", config_str);
    eprintln!("    WorkDir: {}", working_dir);

    let init_content = format!(
        r#"#!{}
name={}
description={}
supervisor=supervise-daemon
command={}
directory={}
pidfile={}
output_log_dir={}
output_log={}
error_log={}
supervise_daemon={}
config_env={}
service_manager_env={}
log_destination_env={}
log_dir_env={}
respawn_delay=5
respawn_max=10
respawn_period=60

depend() {{
    after net
}}

start_pre() {{
    checkpath --directory --mode 0750 "$output_log_dir"
    checkpath --file --mode 0640 "$output_log"
    checkpath --file --mode 0640 "$error_log"
}}

start() {{
    ebegin "Starting ${{RC_SVCNAME}}"
    "$supervise_daemon" "${{RC_SVCNAME}}" \
        --start "$command" \
        --pidfile "$pidfile" \
        --chdir "$directory" \
        --stdout "$output_log" \
        --stderr "$error_log" \
        --respawn-delay "$respawn_delay" \
        --respawn-max "$respawn_max" \
        --respawn-period "$respawn_period" \
        --umask 0077 \
        --env "$config_env" \
        --env "$service_manager_env" \
        --env "$log_destination_env" \
        --env "$log_dir_env"
    eend $?
}}

stop() {{
    ebegin "Stopping ${{RC_SVCNAME}}"
    "$supervise_daemon" "${{RC_SVCNAME}}" --stop "$command" --pidfile "$pidfile"
    eend $?
}}
"#,
        openrc_run_bin(),
        shell_quote(SERVICE_NAME),
        shell_quote("Aether Proxy"),
        shell_quote(exe_str),
        shell_quote(working_dir),
        shell_quote(OPENRC_PID_PATH),
        shell_quote(OPENRC_LOG_DIR),
        shell_quote(OPENRC_STDOUT_LOG),
        shell_quote(OPENRC_STDERR_LOG),
        shell_quote(supervise_daemon_bin()),
        shell_quote(&format!("AETHER_PROXY_CONFIG={config_str}")),
        shell_quote("AETHER_PROXY_SERVICE_MANAGER=openrc"),
        shell_quote("AETHER_PROXY_LOG_DESTINATION=both"),
        shell_quote(&format!("AETHER_PROXY_LOG_DIR={OPENRC_LOG_DIR}")),
    );
    std::fs::write(OPENRC_INIT_PATH, &init_content)?;
    set_mode(OPENRC_INIT_PATH, 0o755)?;

    eprintln!("  Enabling and starting service...");
    run_cmd(openrc_update_bin(), &["add", SERVICE_NAME, "default"])?;
    run_cmd(openrc_service_bin(), &[SERVICE_NAME, "start"])?;

    eprintln!();
    if manager_is_active(ServiceManager::OpenRc) {
        eprintln!("  Service started successfully!");
    } else {
        eprintln!("  Service state is not active yet. Check `sudo ./aether-proxy logs`.");
    }

    print_post_install_commands();
    Ok(())
}

fn uninstall_systemd_service() -> anyhow::Result<()> {
    eprintln!("  Stopping and removing existing service...");
    let _ = Command::new("systemctl")
        .args(["disable", "--now", SERVICE_NAME])
        .status();

    if Path::new(SYSTEMD_UNIT_PATH).exists() {
        std::fs::remove_file(SYSTEMD_UNIT_PATH)?;
        eprintln!("  Removed {}", SYSTEMD_UNIT_PATH);
    }

    run_cmd("systemctl", &["daemon-reload"])?;
    eprintln!("  Service uninstalled.");
    Ok(())
}

fn uninstall_openrc_service() -> anyhow::Result<()> {
    eprintln!("  Stopping and removing existing service...");
    let _ = Command::new(openrc_service_bin())
        .args([SERVICE_NAME, "stop"])
        .status();
    let _ = Command::new(openrc_update_bin())
        .args(["del", SERVICE_NAME, "default"])
        .status();

    if Path::new(OPENRC_INIT_PATH).exists() {
        std::fs::remove_file(OPENRC_INIT_PATH)?;
        eprintln!("  Removed {}", OPENRC_INIT_PATH);
    }

    eprintln!("  Service uninstalled.");
    Ok(())
}

fn start_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd("systemctl", &["start", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "start"]),
    }
}

fn stop_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd("systemctl", &["stop", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "stop"]),
    }
}

fn restart_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd("systemctl", &["restart", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "restart"]),
    }
}

fn manager_status(manager: ServiceManager) -> anyhow::Result<ExitStatus> {
    let status = match manager {
        ServiceManager::Systemd => Command::new("systemctl")
            .args(["status", SERVICE_NAME])
            .status()?,
        ServiceManager::OpenRc => Command::new(openrc_service_bin())
            .args([SERVICE_NAME, "status"])
            .status()?,
    };
    Ok(status)
}

fn manager_is_active(manager: ServiceManager) -> bool {
    match manager {
        ServiceManager::Systemd => {
            Path::new(SYSTEMD_UNIT_PATH).exists()
                && Command::new("systemctl")
                    .args(["is-active", "--quiet", SERVICE_NAME])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
        }
        ServiceManager::OpenRc => {
            Path::new(OPENRC_INIT_PATH).exists()
                && Command::new(openrc_service_bin())
                    .args([SERVICE_NAME, "status"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
        }
    }
}

fn print_post_install_commands() {
    eprintln!();
    eprintln!("  Commands:");
    eprintln!("    ./aether-proxy status          # service status");
    eprintln!("    sudo ./aether-proxy logs       # tail logs");
    eprintln!("    sudo ./aether-proxy restart    # restart");
    eprintln!("    sudo ./aether-proxy stop       # stop");
    eprintln!("    sudo ./aether-proxy uninstall  # remove service");
    eprintln!();
}

fn is_systemd_available() -> bool {
    Path::new("/run/systemd/system").exists()
        && Command::new("systemctl")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
}

fn is_openrc_available() -> bool {
    (Path::new("/run/openrc").exists() || Path::new("/run/openrc/softlevel").exists())
        && has_absolute_candidate(OPENRC_RUN_BINS)
        && has_absolute_candidate(OPENRC_SERVICE_BINS)
        && has_absolute_candidate(OPENRC_UPDATE_BINS)
        && has_absolute_candidate(OPENRC_SUPERVISE_BINS)
}

fn has_absolute_candidate(candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.starts_with('/') && Path::new(candidate).exists())
}

fn openrc_run_bin() -> &'static str {
    pick_bin(OPENRC_RUN_BINS)
}

fn openrc_service_bin() -> &'static str {
    pick_bin(OPENRC_SERVICE_BINS)
}

fn openrc_update_bin() -> &'static str {
    pick_bin(OPENRC_UPDATE_BINS)
}

fn supervise_daemon_bin() -> &'static str {
    pick_bin(OPENRC_SUPERVISE_BINS)
}

fn tail_bin() -> &'static str {
    pick_bin(TAIL_BINS)
}

fn pick_bin(candidates: &[&'static str]) -> &'static str {
    candidates
        .iter()
        .copied()
        .find(|candidate| candidate.starts_with('/') && Path::new(candidate).exists())
        .unwrap_or_else(|| candidates[candidates.len() - 1])
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn touch_log(path: &str) -> anyhow::Result<()> {
    OpenOptions::new().create(true).append(true).open(path)?;
    Ok(())
}

fn set_mode(path: &str, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, perms)?;
    }

    #[cfg(not(unix))]
    let _ = (path, mode);

    Ok(())
}
