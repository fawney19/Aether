//! Service installation and management for `aether-tunnel`.
//!
//! Supports the host-native service manager we currently target:
//! `systemd` on most Linux distributions and `OpenRC` on Alpine.

#[cfg(unix)]
use std::fs::OpenOptions;
use std::io::ErrorKind;
#[cfg(unix)]
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

const SERVICE_NAME: &str = "aether-tunnel";

const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/aether-tunnel.service";

const OPENRC_INIT_PATH: &str = "/etc/init.d/aether-tunnel";
const OPENRC_PID_PATH: &str = "/run/aether-tunnel.pid";
const OPENRC_LOG_DIR: &str = "/var/log/aether-tunnel";
const OPENRC_STDOUT_LOG: &str = "/var/log/aether-tunnel/current.log";
const OPENRC_STDERR_LOG: &str = "/var/log/aether-tunnel/error.log";

const SYSTEMCTL_BINS: &[&str] = &[
    "/usr/bin/systemctl",
    "/bin/systemctl",
    "/run/current-system/sw/bin/systemctl",
];
const JOURNALCTL_BINS: &[&str] = &[
    "/usr/bin/journalctl",
    "/bin/journalctl",
    "/run/current-system/sw/bin/journalctl",
];
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
                "requires root with {}, use: sudo aether-tunnel setup",
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
        anyhow::bail!("root required, use: sudo ./aether-tunnel setup");
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
        ServiceManager::Systemd => Command::new(journalctl_bin())
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
                    "OpenRC logs are stored under {} and usually require root access. Try `sudo ./aether-tunnel logs`.",
                    OPENRC_LOG_DIR
                );
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                anyhow::bail!(
                    "OpenRC log file not found at {}. Start the service first or check `./aether-tunnel status`.",
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
        anyhow::anyhow!("service not installed, run `sudo ./aether-tunnel setup` first")
    })
}

fn ensure_root_and_service() -> anyhow::Result<ServiceManager> {
    let manager = ensure_service_installed()?;
    if !is_root() {
        anyhow::bail!("root required, use: sudo ./aether-tunnel <command>");
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

    validate_service_unit_path(exe_str, "binary")?;
    validate_service_unit_path(config_str, "config")?;
    validate_service_unit_path(working_dir, "working directory")?;
    validate_root_managed_service_file(&exe_path, "binary", false)?;
    validate_root_managed_service_file(&config_abs, "config", true)?;

    if Path::new(SYSTEMD_UNIT_PATH).exists() {
        eprintln!("  Stopping existing service...");
        let _ = Command::new(systemctl_bin())
            .args(["stop", SERVICE_NAME])
            .status();
    }

    eprintln!("  Generating systemd unit file...");
    eprintln!("    Binary:  {}", exe_str);
    eprintln!("    Config:  {}", config_str);
    eprintln!("    WorkDir: {}", working_dir);

    let unit_content = render_systemd_unit(exe_str, config_str, working_dir)?;
    write_service_definition(SYSTEMD_UNIT_PATH, &unit_content, 0o644)?;

    eprintln!("  Enabling and starting service...");
    run_cmd(systemctl_bin(), &["daemon-reload"])?;
    run_cmd(systemctl_bin(), &["enable", "--now", SERVICE_NAME])?;

    eprintln!();
    if manager_is_active(ServiceManager::Systemd) {
        eprintln!("  Service started successfully!");
    } else {
        eprintln!("  Service state is not active yet. Check `sudo ./aether-tunnel logs`.");
    }

    print_post_install_commands();
    Ok(())
}

fn render_systemd_unit(
    exe_path: &str,
    config_path: &str,
    working_dir: &str,
) -> anyhow::Result<String> {
    validate_service_unit_path(exe_path, "binary")?;
    validate_service_unit_path(config_path, "config")?;
    validate_service_unit_path(working_dir, "working directory")?;

    let exe_path = systemd_quote(exe_path);
    let working_dir = systemd_quote(working_dir);
    let config_env = systemd_quote(&format!("AETHER_TUNNEL_CONFIG={config_path}"));
    Ok(format!(
        "[Unit]\n\
         Description=Aether Tunnel\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={working_dir}\n\
         Environment={config_env}\n\
         Environment=AETHER_TUNNEL_SERVICE_MANAGER=systemd\n\
         Environment=AETHER_TUNNEL_LOG_DESTINATION=both\n\
         Environment=AETHER_TUNNEL_LOG_DIR=/var/log/aether-tunnel\n\
         ExecStart={exe_path}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         LimitNOFILE=65535\n\
         UMask=0077\n\
         LogsDirectory=aether-tunnel\n\
         LogsDirectoryMode=0750\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
    ))
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

    validate_service_unit_path(exe_str, "binary")?;
    validate_service_unit_path(config_str, "config")?;
    validate_service_unit_path(working_dir, "working directory")?;
    validate_root_managed_service_file(&exe_path, "binary", false)?;
    validate_root_managed_service_file(&config_abs, "config", true)?;

    if Path::new(OPENRC_INIT_PATH).exists() {
        eprintln!("  Stopping existing service...");
        let _ = Command::new(openrc_service_bin())
            .args([SERVICE_NAME, "stop"])
            .status();
    }

    ensure_private_service_directory(Path::new(OPENRC_LOG_DIR), 0o750)?;
    open_private_service_log(Path::new(OPENRC_STDOUT_LOG), 0o640)?;
    open_private_service_log(Path::new(OPENRC_STDERR_LOG), 0o640)?;

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
        shell_quote("Aether Tunnel"),
        shell_quote(exe_str),
        shell_quote(working_dir),
        shell_quote(OPENRC_PID_PATH),
        shell_quote(OPENRC_LOG_DIR),
        shell_quote(OPENRC_STDOUT_LOG),
        shell_quote(OPENRC_STDERR_LOG),
        shell_quote(supervise_daemon_bin()),
        shell_quote(&format!("AETHER_TUNNEL_CONFIG={config_str}")),
        shell_quote("AETHER_TUNNEL_SERVICE_MANAGER=openrc"),
        shell_quote("AETHER_TUNNEL_LOG_DESTINATION=both"),
        shell_quote(&format!("AETHER_TUNNEL_LOG_DIR={OPENRC_LOG_DIR}")),
    );
    write_service_definition(OPENRC_INIT_PATH, &init_content, 0o755)?;

    eprintln!("  Enabling and starting service...");
    run_cmd(openrc_update_bin(), &["add", SERVICE_NAME, "default"])?;
    run_cmd(openrc_service_bin(), &[SERVICE_NAME, "start"])?;

    eprintln!();
    if manager_is_active(ServiceManager::OpenRc) {
        eprintln!("  Service started successfully!");
    } else {
        eprintln!("  Service state is not active yet. Check `sudo ./aether-tunnel logs`.");
    }

    print_post_install_commands();
    Ok(())
}

fn uninstall_systemd_service() -> anyhow::Result<()> {
    eprintln!("  Stopping and removing existing service...");
    let _ = Command::new(systemctl_bin())
        .args(["disable", "--now", SERVICE_NAME])
        .status();

    if Path::new(SYSTEMD_UNIT_PATH).exists() {
        std::fs::remove_file(SYSTEMD_UNIT_PATH)?;
        eprintln!("  Removed {}", SYSTEMD_UNIT_PATH);
    }

    run_cmd(systemctl_bin(), &["daemon-reload"])?;
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
        ServiceManager::Systemd => run_cmd(systemctl_bin(), &["start", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "start"]),
    }
}

fn stop_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd(systemctl_bin(), &["stop", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "stop"]),
    }
}

fn restart_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd(systemctl_bin(), &["restart", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "restart"]),
    }
}

fn manager_status(manager: ServiceManager) -> anyhow::Result<ExitStatus> {
    let status = match manager {
        ServiceManager::Systemd => Command::new(systemctl_bin())
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
                && Command::new(systemctl_bin())
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
    eprintln!("    ./aether-tunnel status          # service status");
    eprintln!("    sudo ./aether-tunnel logs       # tail logs");
    eprintln!("    sudo ./aether-tunnel restart    # restart");
    eprintln!("    sudo ./aether-tunnel stop       # stop");
    eprintln!("    sudo ./aether-tunnel uninstall  # remove service");
    eprintln!();
}

fn is_systemd_available() -> bool {
    Path::new("/run/systemd/system").exists()
        && has_absolute_candidate(SYSTEMCTL_BINS)
        && Command::new(systemctl_bin())
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
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .find(|candidate| candidate.starts_with('/'))
        })
        .expect("trusted binary candidate list must include an absolute path")
}

fn systemctl_bin() -> &'static str {
    pick_bin(SYSTEMCTL_BINS)
}

fn journalctl_bin() -> &'static str {
    pick_bin(JOURNALCTL_BINS)
}

fn validate_service_unit_path(value: &str, label: &str) -> anyhow::Result<()> {
    if !Path::new(value).is_absolute() {
        anyhow::bail!("{} path must be absolute", label);
    }
    if value.chars().any(char::is_control) || value.contains(['%', '$']) {
        anyhow::bail!(
            "{} path contains control or service-manager expansion characters",
            label
        );
    }
    Ok(())
}

fn validate_root_managed_service_file(
    path: &Path,
    label: &str,
    require_private_file: bool,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let file_metadata = std::fs::symlink_metadata(path)?;
        if !file_metadata.is_file() || file_metadata.file_type().is_symlink() {
            anyhow::bail!("service {} must be a regular non-symlink file", label);
        }
        if file_metadata.uid() != 0 || file_metadata.mode() & 0o022 != 0 {
            anyhow::bail!(
                "service {} must be owned by root and not writable by group or other users; use the official installer or move it to a root-managed path",
                label
            );
        }
        if require_private_file && (file_metadata.mode() & 0o077 != 0 || file_metadata.nlink() != 1)
        {
            anyhow::bail!(
                "service {} contains credentials and must be owner-only with exactly one hard link",
                label
            );
        }

        let mut ancestor = path.parent();
        while let Some(directory) = ancestor {
            let metadata = std::fs::symlink_metadata(directory)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
            {
                anyhow::bail!(
                    "service {} parent '{}' must be a root-owned directory that is not writable by group or other users",
                    label,
                    directory.display()
                );
            }
            ancestor = directory.parent();
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (path, label, require_private_file);
        anyhow::bail!("managed tunnel services require Unix ownership checks")
    }
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

fn write_service_definition(path: &str, content: &str, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let requested_path = Path::new(path);
        let requested_parent = requested_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("service definition path has no parent"))?;
        let file_name = requested_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("service definition path has no file name"))?;
        let parent = std::fs::canonicalize(requested_parent)?;
        let path = parent.join(file_name);
        validate_private_service_directory(&parent)?;
        validate_replaceable_service_file(&path)?;

        let temporary = parent.join(format!(
            ".aether-tunnel-service-{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut file = options.open(&temporary)?;
        let result = (|| -> anyhow::Result<()> {
            // SAFETY: geteuid has no preconditions and does not retain pointers.
            let effective_uid = unsafe { libc::geteuid() };
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.uid() != effective_uid || metadata.nlink() != 1 {
                anyhow::bail!("temporary service definition has unsafe ownership or links");
            }
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &path)?;
            std::fs::File::open(&parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    #[cfg(not(unix))]
    {
        let _ = (path, content, mode);
        anyhow::bail!("managed service definitions require Unix filesystem checks")
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn validate_private_service_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || (metadata.uid() != effective_uid && metadata.uid() != 0)
            || metadata.mode() & 0o022 != 0
        {
            anyhow::bail!(
                "service directory '{}' has unsafe ownership or permissions",
                path.display()
            );
        }

        let canonical = std::fs::canonicalize(path)?;
        let mut ancestor = canonical.parent();
        while let Some(directory) = ancestor {
            let metadata = std::fs::symlink_metadata(directory)?;
            let mode = metadata.mode();
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || (metadata.uid() != effective_uid && metadata.uid() != 0)
                || (mode & 0o022 != 0 && mode & 0o1000 == 0)
            {
                anyhow::bail!(
                    "service directory ancestor '{}' has unsafe ownership or permissions",
                    directory.display()
                );
            }
            ancestor = directory.parent();
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        anyhow::bail!("managed service directories require Unix filesystem checks")
    }
}

fn validate_replaceable_service_file(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                // SAFETY: geteuid has no preconditions and does not retain pointers.
                let effective_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != effective_uid
                    || metadata.nlink() != 1
                {
                    anyhow::bail!(
                        "service file '{}' must be a regular single-link file owned by the current user",
                        path.display()
                    );
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        anyhow::bail!("managed service files require Unix filesystem checks")
    }
}

fn ensure_private_service_directory(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

        let requested_parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("service directory has no parent"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("service directory has no file name"))?;
        let parent = std::fs::canonicalize(requested_parent)?;
        let path = parent.join(file_name);
        validate_private_service_directory(&parent)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let mut builder = std::fs::DirBuilder::new();
                builder.mode(mode).create(&path)?;
            }
            Err(error) => return Err(error.into()),
        }

        let mut options = OpenOptions::new();
        options.read(true).custom_flags(
            libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK,
        );
        let directory = options.open(&path)?;
        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.uid() != effective_uid || metadata.mode() & 0o022 != 0 {
            anyhow::bail!("service log directory has unsafe ownership or permissions");
        }
        directory.set_permissions(std::fs::Permissions::from_mode(mode))?;
        directory.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        anyhow::bail!("managed service directories require Unix filesystem checks")
    }
}

fn open_private_service_log(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let requested_parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("service log path has no parent"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("service log path has no file name"))?;
        let parent = std::fs::canonicalize(requested_parent)?;
        let path = parent.join(file_name);
        validate_private_service_directory(&parent)?;
        let mut options = OpenOptions::new();
        options
            .create(true)
            .append(true)
            .mode(mode)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = options.open(&path)?;
        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != effective_uid || metadata.nlink() != 1 {
            anyhow::bail!("service log must be a regular, single-link file owned by root");
        }
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        anyhow::bail!("managed service logs require Unix filesystem checks")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_private_service_directory, open_private_service_log, pick_bin, render_systemd_unit,
        systemd_quote, validate_root_managed_service_file, validate_service_unit_path,
        write_service_definition,
    };

    #[test]
    fn systemd_unit_quotes_paths_without_changing_arguments() {
        let unit = render_systemd_unit(
            r#"/opt/Aether Tunnel/aether\"tunnel"#,
            r#"/var/lib/aether tunnel/config\\node.toml"#,
            "/var/lib/aether tunnel",
        )
        .expect("safe absolute paths should render");

        assert!(unit.contains(r#"ExecStart="/opt/Aether Tunnel/aether\\\"tunnel""#));
        assert!(unit.contains(
            r#"Environment="AETHER_TUNNEL_CONFIG=/var/lib/aether tunnel/config\\\\node.toml""#
        ));
        assert!(unit.contains(r#"WorkingDirectory="/var/lib/aether tunnel""#));
        assert_eq!(systemd_quote("a\\b\"c"), r#""a\\b\"c""#);
    }

    #[test]
    fn service_unit_paths_reject_directive_and_expansion_injection() {
        for value in [
            "relative/path",
            "/tmp/config\nExecStart=/tmp/evil",
            "/tmp/config\rEnvironment=EVIL=1",
            "/tmp/%n/config",
            "/tmp/$PATH/config",
        ] {
            assert!(
                validate_service_unit_path(value, "test").is_err(),
                "accepted unsafe service path: {value:?}"
            );
        }
    }

    #[test]
    fn service_commands_never_fall_back_to_path_lookup() {
        assert_eq!(
            pick_bin(&["relative-tool", "/definitely/missing/trusted-tool"]),
            "/definitely/missing/trusted-tool"
        );
    }

    #[cfg(unix)]
    #[test]
    fn root_service_rejects_files_beneath_shared_writable_directories() {
        let directory =
            std::env::temp_dir().join(format!("aether-service-path-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).expect("test directory should be created");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777))
                .expect("test directory permissions should be set");
        }
        let path = directory.join("aether-tunnel");
        std::fs::write(&path, b"binary").expect("test binary should be written");

        let result = validate_root_managed_service_file(&path, "binary", false);
        let _ = std::fs::remove_dir_all(&directory);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn service_definitions_and_logs_refuse_links_and_use_private_atomic_files() {
        use std::io::Read;
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

        let directory = std::env::temp_dir().join(format!(
            "aether-service-write-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();

        let definition = directory.join("aether-tunnel.service");
        write_service_definition(definition.to_str().unwrap(), "first", 0o644).unwrap();
        let metadata = std::fs::symlink_metadata(&definition).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o644);
        assert_eq!(metadata.nlink(), 1);
        let mut old_definition = std::fs::File::open(&definition).unwrap();
        write_service_definition(definition.to_str().unwrap(), "second", 0o644).unwrap();
        let mut old_contents = String::new();
        old_definition.read_to_string(&mut old_contents).unwrap();
        assert_eq!(old_contents, "first");
        assert_eq!(std::fs::read_to_string(&definition).unwrap(), "second");

        let victim = directory.join("victim");
        std::fs::write(&victim, b"known-good").unwrap();
        std::fs::remove_file(&definition).unwrap();
        symlink(&victim, &definition).unwrap();
        assert!(write_service_definition(definition.to_str().unwrap(), "replace", 0o644).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_file(&definition).unwrap();
        std::fs::hard_link(&victim, &definition).unwrap();
        assert!(write_service_definition(definition.to_str().unwrap(), "replace", 0o644).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");
        std::fs::remove_file(&definition).unwrap();

        let log_directory = directory.join("logs");
        ensure_private_service_directory(&log_directory, 0o750).unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&log_directory).unwrap().mode() & 0o777,
            0o750
        );
        let log = log_directory.join("current.log");
        open_private_service_log(&log, 0o640).unwrap();
        let metadata = std::fs::symlink_metadata(&log).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o640);
        assert_eq!(metadata.nlink(), 1);
        std::fs::remove_file(&log).unwrap();
        symlink(&victim, &log).unwrap();
        assert!(open_private_service_log(&log, 0o640).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_file(&log).unwrap();
        std::fs::hard_link(&victim, &log).unwrap();
        assert!(open_private_service_log(&log, 0o640).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_dir_all(directory).unwrap();
    }
}
