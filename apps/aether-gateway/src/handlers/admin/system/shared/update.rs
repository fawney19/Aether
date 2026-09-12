use crate::handlers::admin::system::shared::update_client::{
    build_update_http_client, is_trusted_update_url,
};
use crate::GatewayError;
use axum::http;
use futures_util::StreamExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
#[cfg(unix)]
use std::io::{Read, Write};
use std::path::Component;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SystemUpdateTaskStatus {
    pub phase: &'static str,
    pub error: Option<String>,
    pub output: Option<String>,
    pub progress_label: Option<String>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub progress_percent: Option<u8>,
}

static UPDATE_TASK_STATUS: std::sync::OnceLock<Mutex<SystemUpdateTaskStatus>> =
    std::sync::OnceLock::new();

fn update_task_status_lock() -> &'static Mutex<SystemUpdateTaskStatus> {
    UPDATE_TASK_STATUS.get_or_init(|| {
        Mutex::new(SystemUpdateTaskStatus {
            phase: "idle",
            error: None,
            output: None,
            progress_label: None,
            downloaded_bytes: None,
            total_bytes: None,
            progress_percent: None,
        })
    })
}

fn set_update_task_phase(phase: &'static str) {
    if let Ok(mut guard) = update_task_status_lock().lock() {
        guard.phase = phase;
        guard.error = None;
        guard.output = None;
        guard.progress_label = None;
        guard.downloaded_bytes = None;
        guard.total_bytes = None;
        guard.progress_percent = None;
    }
}

fn set_update_task_download_progress(label: &str, downloaded_bytes: u64, total_bytes: Option<u64>) {
    if let Ok(mut guard) = update_task_status_lock().lock() {
        guard.progress_label = Some(label.to_string());
        guard.downloaded_bytes = Some(downloaded_bytes);
        guard.total_bytes = total_bytes;
        guard.progress_percent = total_bytes
            .filter(|total| *total > 0)
            .map(|total| ((downloaded_bytes.saturating_mul(100) / total).min(100)) as u8);
    }
}

fn set_update_task_failed(error: String) {
    if let Ok(mut guard) = update_task_status_lock().lock() {
        guard.phase = "failed";
        guard.error = Some(safe_system_update_error(&error).to_string());
    }
}

fn set_update_task_output(_output: String) {
    if let Ok(mut guard) = update_task_status_lock().lock() {
        guard.output = Some("Update package prepared".to_string());
    }
}

pub(crate) fn read_update_task_status() -> SystemUpdateTaskStatus {
    let mut status = update_task_status_lock()
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or(SystemUpdateTaskStatus {
            phase: "idle",
            error: None,
            output: None,
            progress_label: None,
            downloaded_bytes: None,
            total_bytes: None,
            progress_percent: None,
        });
    status.error = status
        .error
        .as_deref()
        .map(|error| safe_system_update_error(error).to_string());
    status.output = status
        .output
        .as_ref()
        .map(|_| "System update step completed".to_string());
    status
}

static PREPARED_VERSION: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
static UPDATE_HISTORY_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

fn prepared_version_lock() -> &'static Mutex<Option<String>> {
    PREPARED_VERSION.get_or_init(|| Mutex::new(None))
}

fn set_prepared_version(version: String) {
    if let Ok(mut guard) = prepared_version_lock().lock() {
        *guard = Some(version);
    }
}

fn clear_prepared_version() {
    if let Ok(mut guard) = prepared_version_lock().lock() {
        *guard = None;
    }
}

pub(crate) fn get_prepared_version() -> Option<String> {
    prepared_version_lock().lock().ok()?.clone()
}

const UPDATE_HISTORY_FILENAME: &str = ".aether-update-history.json";
const PREVIOUS_RELEASE_FILENAME: &str = ".aether-previous-release";
const MAX_HISTORY_ENTRIES: usize = 50;
const MAX_UPDATE_HISTORY_BYTES: usize = 256 * 1024;
const MAX_PREVIOUS_RELEASE_BYTES: usize = 256;
const RESTART_EXIT_CODE: i32 = 75;
const MAX_RELEASE_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SHA256SUMS_DOWNLOAD_BYTES: u64 = 1024 * 1024;
const MAX_EXTRACTED_RELEASE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RELEASE_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_RELEASE_ARCHIVE_PATH_DEPTH: usize = 64;
const DEFAULT_UPDATE_DOWNLOAD_TIMEOUT_SECS: u64 = 600;
const DEFAULT_UPDATE_DOWNLOAD_IDLE_TIMEOUT_SECS: u64 = 30;
const SOURCE_BUILD_UPDATE_BLOCKER: &str = "当前为源码构建，请使用 git pull 后重新编译。";
const DOCKER_UPDATE_BLOCKER: &str =
    "Docker 部署请使用镜像更新：进入 docker-compose.yml 所在目录执行 ./update.sh。";
const MANUAL_UPDATE_BLOCKER: &str =
    "当前部署策略不支持在线自更新，请手动下载 Release 或使用安装脚本更新。";
const MULTI_NODE_UPDATE_BLOCKER: &str =
    "多节点部署不支持在管理后台更新单个节点，请使用镜像滚动更新或外部发布编排。";
const STORAGE_UPDATE_BLOCKER: &str =
    "当前安装目录未提供安全的自更新写权限，请使用安装脚本或受限的系统更新服务。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateStrategy {
    SelfManaged,
    Docker,
    Manual,
}

impl UpdateStrategy {
    fn from_env_value(value: Option<&str>, release_build: bool) -> Self {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return if release_build {
                Self::SelfManaged
            } else {
                Self::Manual
            };
        };

        match value.to_ascii_lowercase().as_str() {
            "self" | "self-managed" | "binary" | "systemd" | "launchd" => Self::SelfManaged,
            "docker" | "compose" | "docker-compose" | "container" => Self::Docker,
            "manual" | "source" | "none" | "off" | "disabled" => Self::Manual,
            _ => Self::Manual,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SelfManaged => "self",
            Self::Docker => "docker",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeploymentTopology {
    SingleNode,
    MultiNode,
}

impl DeploymentTopology {
    fn from_env_value(value: Option<&str>) -> Self {
        match value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("multi-node" | "multi" | "cluster") => Self::MultiNode,
            _ => Self::SingleNode,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::SingleNode => "single-node",
            Self::MultiNode => "multi-node",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UpdateHistoryEntry {
    pub timestamp: String,
    pub operation: String,
    pub success: bool,
    pub error: Option<String>,
    pub output_tail: Option<String>,
}

fn aether_base_dir() -> PathBuf {
    std::env::var("AETHER_BASE_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/aether"))
}

fn releases_base_dir() -> PathBuf {
    aether_base_dir().join("releases")
}

fn safe_release_name(version: &str) -> Result<String, String> {
    let value = version.trim();
    if value.is_empty() || value == "." || value == ".." {
        return Err("版本号为空或非法".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '+'))
    {
        return Err(format!("版本号包含非法字符: {version}"));
    }
    Ok(value.to_string())
}

fn release_dir_for_version(version: &str) -> Result<PathBuf, String> {
    Ok(releases_base_dir().join(safe_release_name(version)?))
}

fn current_release_name() -> Option<String> {
    current_release_name_at(&aether_base_dir())
}

fn current_release_name_at(base_dir: &Path) -> Option<String> {
    let releases = std::fs::canonicalize(base_dir.join("releases")).ok()?;
    let current = base_dir.join("current");
    if !std::fs::symlink_metadata(&current)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let target = std::fs::canonicalize(current).ok()?;
    let relative = target.strip_prefix(releases).ok()?;
    let mut components = relative.components();
    let Component::Normal(name) = components.next()? else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    let name = name.to_str()?;
    safe_release_name(name).ok()
}

fn update_history_path() -> PathBuf {
    aether_base_dir().join(UPDATE_HISTORY_FILENAME)
}

fn update_history_lock() -> &'static Mutex<()> {
    UPDATE_HISTORY_LOCK.get_or_init(|| Mutex::new(()))
}

fn append_update_history(
    operation: &str,
    success: bool,
    error: Option<&str>,
    output: Option<&str>,
) {
    let path = update_history_path();
    let _guard = update_history_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    append_update_history_at_path(&path, operation, success, error, output);
}

fn append_update_history_at_path(
    path: &Path,
    operation: &str,
    success: bool,
    error: Option<&str>,
    output: Option<&str>,
) {
    let entry = UpdateHistoryEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        operation: safe_system_update_operation(operation).to_string(),
        success,
        error: error.map(|error| safe_system_update_error(error).to_string()),
        output_tail: output.map(|_| safe_system_update_output(operation).to_string()),
    };

    let (mut entries, _) = load_and_sanitize_update_history(path);

    entries.push(entry);
    sanitize_update_history_entries(&mut entries);
    persist_update_history(path, &entries);
}

pub(crate) fn read_update_history() -> Vec<UpdateHistoryEntry> {
    let path = update_history_path();
    let _guard = update_history_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_update_history_at_path(&path)
}

fn read_update_history_at_path(path: &Path) -> Vec<UpdateHistoryEntry> {
    let (entries, changed) = load_and_sanitize_update_history(path);
    if changed {
        persist_update_history(path, &entries);
    }
    entries
}

fn load_and_sanitize_update_history(path: &Path) -> (Vec<UpdateHistoryEntry>, bool) {
    let Ok(Some(content)) = read_update_metadata_file(path, MAX_UPDATE_HISTORY_BYTES) else {
        return (Vec::new(), false);
    };
    let Ok(content) = String::from_utf8(content) else {
        return (Vec::new(), true);
    };
    let Ok(mut entries) = serde_json::from_str::<Vec<UpdateHistoryEntry>>(&content) else {
        return (Vec::new(), !content.trim().is_empty());
    };
    let changed = sanitize_update_history_entries(&mut entries);
    (entries, changed)
}

fn sanitize_update_history_entries(entries: &mut Vec<UpdateHistoryEntry>) -> bool {
    // Deliberately use a non-short-circuiting fold: every historical entry
    // must be sanitized even after one entry changes.
    #[allow(clippy::unnecessary_fold)]
    let mut changed = entries.iter_mut().fold(false, |changed, entry| {
        sanitize_update_history_entry(entry) || changed
    });
    if entries.len() > MAX_HISTORY_ENTRIES {
        entries.drain(..entries.len() - MAX_HISTORY_ENTRIES);
        changed = true;
    }
    changed
}

fn sanitize_update_history_entry(entry: &mut UpdateHistoryEntry) -> bool {
    let mut changed = false;

    if chrono::DateTime::parse_from_rfc3339(&entry.timestamp).is_err() {
        entry.timestamp = "1970-01-01T00:00:00Z".to_string();
        changed = true;
    }

    let operation = safe_system_update_operation(&entry.operation).to_string();
    if entry.operation != operation {
        entry.operation = operation;
        changed = true;
    }

    let error = entry
        .error
        .as_deref()
        .map(|error| safe_system_update_error(error).to_string());
    if entry.error != error {
        entry.error = error;
        changed = true;
    }

    let output_tail = entry
        .output_tail
        .as_ref()
        .map(|_| safe_system_update_output(&entry.operation).to_string());
    if entry.output_tail != output_tail {
        entry.output_tail = output_tail;
        changed = true;
    }

    changed
}

fn persist_update_history(path: &Path, entries: &[UpdateHistoryEntry]) {
    let result = serde_json::to_vec_pretty(entries)
        .map_err(|err| format!("序列化更新历史失败: {err}"))
        .and_then(|json| write_update_metadata_atomic(path, &json));
    if let Err(err) = result {
        tracing::warn!(error = %safe_system_update_error(&err), "failed to persist update history");
    }
}

fn read_update_metadata_file(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::fs::MetadataExt;

        let (parent, file_name) = open_real_update_parent(path)?;
        let file_name = unix_update_path_component(&file_name, "更新元数据文件名")?;
        // SAFETY: parent is a live directory descriptor, file_name is NUL-terminated, and a
        // successful descriptor is transferred immediately into File.
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                file_name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                return Ok(None);
            }
            return Err(format!("安全打开更新元数据失败: {error}"));
        }
        // SAFETY: openat returned a new owned descriptor.
        let mut file = unsafe { std::fs::File::from_raw_fd(descriptor) };
        let metadata = file
            .metadata()
            .map_err(|err| format!("读取更新元数据属性失败: {err}"))?;
        if !metadata.is_file() {
            return Err("更新元数据必须是普通文件".to_string());
        }
        // SAFETY: geteuid has no preconditions and retains no pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid || metadata.mode() & 0o022 != 0 {
            return Err("更新元数据所有者或写权限不安全".to_string());
        }
        if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
            return Err("更新元数据超过大小限制".to_string());
        }
        let mut bytes = Vec::with_capacity((metadata.len() as usize).min(max_bytes));
        Read::by_ref(&mut file)
            .take(
                u64::try_from(max_bytes)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_to_end(&mut bytes)
            .map_err(|err| format!("读取更新元数据失败: {err}"))?;
        if bytes.len() > max_bytes {
            return Err("更新元数据超过大小限制".to_string());
        }
        Ok(Some(bytes))
    }

    #[cfg(not(unix))]
    {
        let _ = (path, max_bytes);
        Err("当前平台不支持安全读取自更新元数据".to_string())
    }
}

fn write_update_metadata_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let (parent, output_file_name) = open_real_update_parent(path)?;
        let parent_metadata = parent
            .metadata()
            .map_err(|err| format!("读取更新元数据目录属性失败: {err}"))?;
        // SAFETY: geteuid has no preconditions and retains no pointers.
        let effective_uid = unsafe { libc::geteuid() };
        let parent_mode = parent_metadata.mode();
        if parent_metadata.uid() != effective_uid
            || parent_mode & 0o022 != 0
            || ((parent_mode >> 6) & 0o3) != 0o3
        {
            return Err("更新元数据目录所有者或写权限不安全".to_string());
        }

        let output_name = unix_update_path_component(&output_file_name, "更新元数据文件名")?;
        if let Some(stat) = unix_update_file_stat_at(&parent, &output_name)? {
            if stat.st_mode & libc::S_IFMT != libc::S_IFREG
                || stat.st_uid != effective_uid
                || stat.st_mode & 0o022 != 0
            {
                return Err("已有更新元数据不是安全的普通文件".to_string());
            }
        }

        let temp_file_name = std::ffi::OsString::from(format!(
            ".aether-update-metadata-{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let temp_name = unix_update_path_component(&temp_file_name, "临时更新元数据文件名")?;
        // SAFETY: parent and temp_name remain valid for the call. O_EXCL prevents collisions and
        // the successful descriptor is transferred immediately into File.
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temp_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(format!(
                "创建临时更新元数据失败: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: openat returned a new owned descriptor.
        let mut temp = unsafe { std::fs::File::from_raw_fd(descriptor) };
        let result = (|| -> Result<(), String> {
            temp.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|err| format!("设置更新元数据权限失败: {err}"))?;
            temp.write_all(bytes)
                .map_err(|err| format!("写入更新元数据失败: {err}"))?;
            temp.sync_all()
                .map_err(|err| format!("同步更新元数据失败: {err}"))?;
            drop(temp);
            unix_update_rename_at(&parent, &temp_name, &output_name)?;
            parent
                .sync_all()
                .map_err(|err| format!("同步更新元数据目录失败: {err}"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = unix_update_unlink_at(&parent, &temp_name);
        }
        result
    }

    #[cfg(not(unix))]
    {
        let _ = (path, bytes);
        Err("当前平台不支持安全写入自更新元数据".to_string())
    }
}

#[cfg(unix)]
fn open_real_update_parent(path: &Path) -> Result<(std::fs::File, std::ffi::OsString), String> {
    let file_name = path
        .file_name()
        .map(std::ffi::OsString::from)
        .ok_or_else(|| "更新元数据路径缺少文件名".to_string())?;
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let canonical_parent =
        std::fs::canonicalize(parent).map_err(|err| format!("解析更新元数据目录失败: {err}"))?;
    Ok((open_real_update_directory(&canonical_parent)?, file_name))
}

#[cfg(unix)]
fn open_real_update_directory(path: &Path) -> Result<std::fs::File, String> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let mut directory = std::fs::File::open(if path.is_absolute() { "/" } else { "." })
        .map_err(|err| format!("打开更新元数据根目录失败: {err}"))?;
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir | Component::Prefix(_) => {
                return Err("更新元数据目录包含不安全路径组件".to_string())
            }
        };
        let name = unix_update_path_component(name, "更新元数据目录组件")?;
        // SAFETY: directory is a live directory descriptor and name is NUL-terminated. A
        // successful descriptor is transferred immediately into File.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if descriptor < 0 {
            return Err(format!(
                "更新元数据目录必须存在且不能经过符号链接: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: openat returned a new owned descriptor.
        directory = unsafe { std::fs::File::from_raw_fd(descriptor) };
    }
    Ok(directory)
}

#[cfg(unix)]
fn unix_update_path_component(
    component: &std::ffi::OsStr,
    description: &str,
) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(component.as_bytes()).map_err(|_| format!("{description} 包含 NUL 字节"))
}

#[cfg(unix)]
fn unix_update_file_stat_at(
    parent: &std::fs::File,
    file_name: &std::ffi::CStr,
) -> Result<Option<libc::stat>, String> {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;

    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: parent and file_name remain valid, and stat points to writable storage initialized
    // only after a successful fstatat call.
    let status = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            file_name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        // SAFETY: successful fstatat initialized the complete stat value.
        return Ok(Some(unsafe { stat.assume_init() }));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOENT) {
        Ok(None)
    } else {
        Err(format!("检查更新元数据目标失败: {error}"))
    }
}

#[cfg(unix)]
fn unix_update_rename_at(
    parent: &std::fs::File,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    // SAFETY: parent is live and both names are NUL-terminated components retained for the call.
    let status = unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "原子替换更新元数据失败: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(unix)]
fn unix_update_unlink_at(parent: &std::fs::File, file_name: &std::ffi::CStr) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    // SAFETY: parent is live and file_name is a NUL-terminated component retained for the call.
    let status = unsafe { libc::unlinkat(parent.as_raw_fd(), file_name.as_ptr(), 0) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "删除临时更新元数据失败: {}",
            std::io::Error::last_os_error()
        ))
    }
}

fn remove_update_metadata_file(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;

        let (parent, file_name) = open_real_update_parent(path)?;
        let file_name = unix_update_path_component(&file_name, "更新元数据文件名")?;
        let Some(stat) = unix_update_file_stat_at(&parent, &file_name)? else {
            return Ok(());
        };
        // SAFETY: geteuid has no preconditions and retains no pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_uid != effective_uid
            || stat.st_mode & 0o022 != 0
        {
            return Err("拒绝删除不安全的更新元数据".to_string());
        }
        // SAFETY: parent is live and file_name is a NUL-terminated component retained for the
        // call. unlinkat removes the directory entry itself and never follows it.
        let status = unsafe { libc::unlinkat(parent.as_raw_fd(), file_name.as_ptr(), 0) };
        if status != 0 {
            return Err(format!(
                "删除更新元数据失败: {}",
                std::io::Error::last_os_error()
            ));
        }
        parent
            .sync_all()
            .map_err(|err| format!("同步更新元数据目录失败: {err}"))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Err("当前平台不支持安全删除自更新元数据".to_string())
    }
}

fn safe_system_update_operation(operation: &str) -> &'static str {
    match operation.trim() {
        "prepare" => "prepare",
        "apply" => "apply",
        "rollback" => "rollback",
        _ => "unknown",
    }
}

fn safe_system_update_output(operation: &str) -> &'static str {
    match safe_system_update_operation(operation) {
        "prepare" => "Update package prepared",
        "apply" => "System update applied",
        "rollback" => "System rollback applied",
        _ => "System update step completed",
    }
}

fn safe_system_update_error(error: &str) -> &'static str {
    let lowered = error.trim().to_ascii_lowercase();
    if lowered.contains("timeout") || lowered.contains("timed out") || lowered.contains("超时") {
        "System update timed out"
    } else if lowered.contains("sha256") || lowered.contains("checksum") || lowered.contains("校验")
    {
        "Update package checksum verification failed"
    } else if [
        "download",
        "http",
        "connect",
        "connection",
        "dns",
        "tls",
        "certificate",
        "下载",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
    {
        "Update package download failed"
    } else if ["archive", "extract", "unpack", "gzip", "tar", "解压"]
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        "Update package extraction failed"
    } else if [
        "symlink",
        "rename",
        "permission",
        "directory",
        "filesystem",
        "install",
        "符号链接",
        "目录",
        "切换",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
    {
        "Update installation failed"
    } else if lowered.contains("utf-8") || lowered.contains("invalid") || lowered.contains("无效")
    {
        "Update package is invalid"
    } else {
        "System update failed"
    }
}

static SYSTEM_UPDATE_RUNNING: AtomicBool = AtomicBool::new(false);

struct SystemUpdateGuard;

impl SystemUpdateGuard {
    fn try_acquire() -> Option<Self> {
        if SYSTEM_UPDATE_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(Self)
        } else {
            None
        }
    }
}

impl Drop for SystemUpdateGuard {
    fn drop(&mut self) {
        SYSTEM_UPDATE_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn current_build_type() -> &'static str {
    option_env!("AETHER_BUILD_TYPE").unwrap_or("source")
}

#[cfg(not(test))]
fn is_release_build() -> bool {
    current_build_type() == "release"
}

#[cfg(test)]
fn is_release_build() -> bool {
    true
}

pub(crate) fn current_update_strategy() -> UpdateStrategy {
    UpdateStrategy::from_env_value(
        std::env::var("AETHER_UPDATE_STRATEGY").ok().as_deref(),
        is_release_build(),
    )
}

fn current_deployment_topology() -> DeploymentTopology {
    DeploymentTopology::from_env_value(
        std::env::var("AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY")
            .ok()
            .as_deref(),
    )
}

fn self_update_supported_for(
    release_build: bool,
    update_strategy: UpdateStrategy,
    deployment_topology: DeploymentTopology,
) -> bool {
    release_build
        && update_strategy == UpdateStrategy::SelfManaged
        && deployment_topology == DeploymentTopology::SingleNode
}

pub(crate) fn self_update_supported() -> bool {
    self_update_supported_for(
        is_release_build(),
        current_update_strategy(),
        current_deployment_topology(),
    ) && self_update_storage_ready()
}

pub(crate) fn current_self_update_blocker() -> &'static str {
    if !is_release_build() {
        return SOURCE_BUILD_UPDATE_BLOCKER;
    }
    if current_deployment_topology() == DeploymentTopology::MultiNode {
        return MULTI_NODE_UPDATE_BLOCKER;
    }

    match current_update_strategy() {
        UpdateStrategy::SelfManaged if self_update_storage_ready() => "一键更新可用",
        UpdateStrategy::SelfManaged => STORAGE_UPDATE_BLOCKER,
        UpdateStrategy::Docker => DOCKER_UPDATE_BLOCKER,
        UpdateStrategy::Manual => MANUAL_UPDATE_BLOCKER,
    }
}

fn self_update_storage_ready() -> bool {
    self_update_storage_ready_at(&aether_base_dir())
}

fn self_update_storage_ready_at(base_dir: &Path) -> bool {
    if !base_dir.is_absolute()
        || base_dir.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
    {
        return false;
    }

    let releases_dir = base_dir.join("releases");
    if !is_safe_writable_update_directory(base_dir)
        || !is_safe_writable_update_directory(&releases_dir)
    {
        return false;
    }

    let Ok(base_canonical) = std::fs::canonicalize(base_dir) else {
        return false;
    };
    if base_canonical != base_dir {
        return false;
    }
    let Ok(releases_canonical) = std::fs::canonicalize(&releases_dir) else {
        return false;
    };
    if releases_canonical.parent() != Some(base_canonical.as_path()) {
        return false;
    }

    let current = base_dir.join("current");
    let Ok(current_metadata) = std::fs::symlink_metadata(&current) else {
        return false;
    };
    if !current_metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(current_canonical) = std::fs::canonicalize(&current) else {
        return false;
    };
    let Ok(relative) = current_canonical.strip_prefix(&releases_canonical) else {
        return false;
    };
    let mut components = relative.components();
    let Some(Component::Normal(release_name)) = components.next() else {
        return false;
    };
    components.next().is_none()
        && release_name
            .to_str()
            .is_some_and(|name| safe_release_name(name).is_ok())
}

fn is_safe_writable_update_directory(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let mode = metadata.mode();
        if mode & 0o022 != 0 {
            return false;
        }
        // SAFETY: these libc calls take no pointers and have no preconditions.
        let effective_uid = unsafe { libc::geteuid() };
        metadata.uid() == effective_uid && ((mode >> 6) & 0o3 == 0o3)
    }

    #[cfg(not(unix))]
    {
        false
    }
}

pub(crate) fn build_admin_system_update_capability_payload() -> serde_json::Value {
    let build_type = current_build_type();
    let update_strategy = current_update_strategy();
    let deployment_topology = current_deployment_topology();
    let supported =
        self_update_supported_for(is_release_build(), update_strategy, deployment_topology)
            && self_update_storage_ready();
    let rollback_available = supported && find_rollback_target().is_some();
    let task_status = read_update_task_status();
    let docker_command = if update_strategy == UpdateStrategy::Docker {
        Some("./update.sh")
    } else {
        None
    };
    json!({
        "supported": supported,
        "enabled": supported,
        "rollback_available": rollback_available,
        "task_status": task_status.phase,
        "task_error": task_status.error,
        "build_type": build_type,
        "update_strategy": update_strategy.as_str(),
        "strategy": update_strategy.as_str(),
        "deployment_topology": deployment_topology.as_str(),
        "topology": deployment_topology.as_str(),
        "docker_update_command": docker_command,
        "message": if supported {
            "一键更新可用"
        } else {
            current_self_update_blocker()
        },
    })
}

fn find_rollback_target() -> Option<String> {
    find_rollback_target_at(&aether_base_dir())
}

fn find_rollback_target_at(base_dir: &Path) -> Option<String> {
    let previous_path = base_dir.join(PREVIOUS_RELEASE_FILENAME);
    let previous = read_update_metadata_file(&previous_path, MAX_PREVIOUS_RELEASE_BYTES).ok()??;
    let previous = std::str::from_utf8(&previous).ok()?.trim();
    let previous = safe_release_name(previous).ok()?;
    let target_dir = base_dir.join("releases").join(&previous);
    validate_release_payload_dir(&target_dir).ok()?;
    Some(previous)
}

pub(crate) async fn prepare_admin_system_update_task(
    version: String,
    tarball_url: String,
    sha256sums_url: Option<String>,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    if !self_update_supported() {
        return Ok(Err(self_update_rejection_response()));
    }
    let Some(sha256sums_url) = sha256sums_url.filter(|url| !url.trim().is_empty()) else {
        return Ok(Err((
            http::StatusCode::PRECONDITION_REQUIRED,
            json!({ "detail": "缺少 SHA256SUMS 校验文件，已拒绝在线更新" }),
        )));
    };
    if validate_update_release_urls(&version, &tarball_url, &sha256sums_url).is_err() {
        return Ok(Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": "更新资产 URL 与官方版本或当前平台不匹配" }),
        )));
    }
    let Some(guard) = SystemUpdateGuard::try_acquire() else {
        return Ok(Err(update_already_running_response()));
    };

    clear_prepared_version();
    set_update_task_phase("preparing");

    tokio::spawn(async move {
        let _guard = guard;
        let result = download_and_extract_release(&version, &tarball_url, &sha256sums_url).await;

        match result {
            Ok(output) => {
                set_update_task_phase("prepared");
                set_update_task_output(output.clone());
                set_prepared_version(version);
                append_update_history("prepare", true, None, Some(&output));
            }
            Err(err) => {
                set_update_task_failed(err.clone());
                append_update_history("prepare", false, Some(&err), None);
            }
        }
    });

    Ok(Ok(json!({
        "message": "更新包开始下载，请等待准备完成",
        "started": true,
        "need_restart": false,
    })))
}

async fn download_and_extract_release(
    version: &str,
    tarball_url: &str,
    sha256sums_url: &str,
) -> Result<String, String> {
    validate_update_release_urls(version, tarball_url, sha256sums_url)?;
    let total_timeout = update_download_total_timeout();
    let client = build_update_http_client(total_timeout, "更新下载")?;
    let (tarball_bytes, sha256_text) = tokio::time::timeout(total_timeout, async {
        set_update_task_phase("downloading");
        let tarball_bytes =
            download_update_bytes(&client, tarball_url, MAX_RELEASE_DOWNLOAD_BYTES, "更新包")
                .await?;

        set_update_task_phase("downloading_checksum");
        let sha256_text = String::from_utf8(
            download_update_bytes(
                &client,
                sha256sums_url,
                MAX_SHA256SUMS_DOWNLOAD_BYTES,
                "校验文件",
            )
            .await?,
        )
        .map_err(|_| "校验文件不是有效 UTF-8".to_string())?;
        Ok::<_, String>((tarball_bytes, sha256_text))
    })
    .await
    .map_err(|_| format!("下载更新包超时: 超过 {} 秒", total_timeout.as_secs()))??;

    let tarball_url_owned = tarball_url.to_string();
    let version_owned = version.to_string();
    tokio::task::spawn_blocking(move || {
        set_update_task_phase("verifying");
        verify_sha256(&tarball_bytes, &sha256_text, &tarball_url_owned)?;
        set_update_task_phase("extracting");
        extract_release(&version_owned, &tarball_bytes)
    })
    .await
    .map_err(|_| "\u{89e3}\u{538b}\u{4efb}\u{52a1}\u{5f02}\u{5e38}".to_string())?
}

async fn download_update_bytes(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
    label: &str,
) -> Result<Vec<u8>, String> {
    validate_update_download_url(url)?;
    let idle_timeout = update_download_idle_timeout();

    let response = tokio::time::timeout(
        idle_timeout,
        client
            .get(url)
            .header(reqwest::header::USER_AGENT, "Aether-Gateway update")
            .send(),
    )
    .await
    .map_err(|_| {
        format!(
            "下载{label}超时: {} 秒内没有收到响应",
            idle_timeout.as_secs()
        )
    })?
    .map_err(|_| format!("下载{label}失败: 网络连接错误"))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载{label}返回错误状态: {}",
            response.status().as_u16()
        ));
    }

    if let Some(content_length) = response.content_length() {
        if content_length > max_bytes {
            return Err(format!(
                "{label}过大: {content_length} bytes，最大允许 {max_bytes} bytes"
            ));
        }
    }

    let total_bytes = response.content_length();
    let mut stream = response.bytes_stream();
    let mut data = Vec::new();
    set_update_task_download_progress(label, 0, total_bytes);
    while let Some(chunk) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            format!(
                "下载{label}超时: {} 秒内没有收到数据",
                idle_timeout.as_secs()
            )
        })?
    {
        let chunk = chunk.map_err(|_| format!("读取{label}数据失败"))?;
        let next_len = data.len() as u64 + chunk.len() as u64;
        if next_len > max_bytes {
            return Err(format!("{label}超过大小限制: 最大允许 {max_bytes} bytes"));
        }
        data.extend_from_slice(&chunk);
        set_update_task_download_progress(label, data.len() as u64, total_bytes);
    }

    Ok(data)
}

fn update_download_total_timeout() -> std::time::Duration {
    update_timeout_from_env(
        "AETHER_UPDATE_DOWNLOAD_TIMEOUT_SECS",
        DEFAULT_UPDATE_DOWNLOAD_TIMEOUT_SECS,
    )
}

fn update_download_idle_timeout() -> std::time::Duration {
    update_timeout_from_env(
        "AETHER_UPDATE_DOWNLOAD_IDLE_TIMEOUT_SECS",
        DEFAULT_UPDATE_DOWNLOAD_IDLE_TIMEOUT_SECS,
    )
}

fn update_timeout_from_env(key: &str, default_secs: u64) -> std::time::Duration {
    let secs = std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_secs);
    std::time::Duration::from_secs(secs)
}

fn validate_update_download_url(raw_url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(raw_url).map_err(|_| "下载 URL 无效".to_string())?;
    if is_trusted_update_url(&parsed) {
        return Ok(());
    }
    Err("下载 URL 必须使用无凭据的 HTTPS GitHub 发布主机".to_string())
}

fn validate_update_release_urls(
    version: &str,
    tarball_url: &str,
    sha256sums_url: &str,
) -> Result<(), String> {
    let safe_version = safe_release_name(version)?;
    let platform = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return Err("当前平台没有受支持的官方更新资产".to_string());
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "amd64"
    } else {
        return Err("当前架构没有受支持的官方更新资产".to_string());
    };
    let asset_name = format!("aether-{safe_version}-{platform}-{arch}.tar.gz");
    let release_prefix = format!("/fawney19/Aether/releases/download/{safe_version}/");

    for (raw_url, expected_name) in [
        (tarball_url, asset_name.as_str()),
        (sha256sums_url, "SHA256SUMS"),
    ] {
        let parsed = url::Url::parse(raw_url).map_err(|_| "更新资产 URL 无效".to_string())?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || !parsed
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
            || parsed.port().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != format!("{release_prefix}{expected_name}")
        {
            return Err("更新资产 URL 必须精确指向官方同版本发布资产".to_string());
        }
    }
    Ok(())
}

fn verify_sha256(data: &[u8], sums_text: &str, tarball_url: &str) -> Result<(), String> {
    let tarball_filename = tarball_url.rsplit('/').next().ok_or_else(|| {
        "\u{65e0}\u{6cd5}\u{4ece} URL \u{63d0}\u{53d6}\u{6587}\u{4ef6}\u{540d}".to_string()
    })?;

    let expected_hashes = sums_text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let (Some(hash), Some(name)) = (fields.next(), fields.next()) else {
                return None;
            };
            if fields.next().is_some() {
                return None;
            }
            let name = name.strip_prefix('*').unwrap_or(name);
            (name == tarball_filename
                && hash.len() == 64
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    let expected_hash = match expected_hashes.as_slice() {
        [hash] => hash,
        [] => {
            return Err(format!(
                "SHA256SUMS \u{4e2d}\u{672a}\u{627e}\u{5230} {tarball_filename} \u{7684}\u{552f}\u{4e00}\u{6709}\u{6548}\u{6821}\u{9a8c}\u{503c}"
            ));
        }
        _ => {
            return Err(format!(
                "SHA256SUMS \u{4e2d} {tarball_filename} \u{5b58}\u{5728}\u{591a}\u{4e2a}\u{6709}\u{6548}\u{6821}\u{9a8c}\u{503c}"
            ));
        }
    };

    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();
    let actual_hash: String = hash.iter().map(|b| format!("{b:02x}")).collect();

    if actual_hash != *expected_hash {
        return Err(format!(
            "SHA256 \u{6821}\u{9a8c}\u{5931}\u{8d25}: \u{671f}\u{671b} {expected_hash}, \u{5b9e}\u{9645} {actual_hash}"
        ));
    }
    Ok(())
}

fn extract_release(version: &str, tarball_bytes: &[u8]) -> Result<String, String> {
    let safe_version = safe_release_name(version)?;
    if current_release_name().as_deref() == Some(safe_version.as_str()) {
        return Err(format!("版本 {version} 已经是当前运行版本"));
    }

    let base_dir = releases_base_dir();
    std::fs::create_dir_all(&base_dir).map_err(|err| {
        format!("\u{521b}\u{5efa} releases \u{76ee}\u{5f55}\u{5931}\u{8d25}: {err}")
    })?;

    let release_dir = base_dir.join(&safe_version);
    let staging_dir = create_release_staging_dir(&base_dir, &safe_version)?;

    if let Err(err) = unpack_release_archive(tarball_bytes, &staging_dir) {
        let _ = remove_path_if_exists(&staging_dir);
        return Err(err);
    }

    let bundle_dir = match find_release_payload_dir(&staging_dir) {
        Ok(dir) => dir,
        Err(err) => {
            let _ = remove_path_if_exists(&staging_dir);
            return Err(err);
        }
    };
    if let Err(err) = validate_release_payload_dir(&bundle_dir) {
        let _ = remove_path_if_exists(&staging_dir);
        return Err(err);
    }

    if let Err(err) = ensure_release_binary_permissions(&bundle_dir.join("bin/aether-gateway")) {
        let _ = remove_path_if_exists(&staging_dir);
        return Err(err);
    }
    if let Err(err) = sync_release_tree(&bundle_dir) {
        let _ = remove_path_if_exists(&staging_dir);
        return Err(err);
    }

    let source_dir = if bundle_dir == staging_dir {
        staging_dir.clone()
    } else {
        bundle_dir
    };
    if let Err(err) = rename_release_dir_noreplace(&source_dir, &release_dir) {
        let _ = remove_path_if_exists(&staging_dir);
        return Err(err);
    }
    let sync_result = sync_update_directory(&base_dir);
    let cleanup_result = if source_dir != staging_dir {
        std::fs::remove_dir(&staging_dir)
    } else {
        Ok(())
    };
    sync_result.map_err(|err| format!("同步版本目录失败: {err}"))?;
    cleanup_result.map_err(|err| format!("清理临时版本目录失败: {err}"))?;

    Ok(format!(
        "\u{7248}\u{672c} {} \u{5df2}\u{51c6}\u{5907}\u{5c31}\u{7eea}",
        version
    ))
}

fn create_release_staging_dir(base_dir: &Path, safe_version: &str) -> Result<PathBuf, String> {
    for _ in 0..16 {
        let path = base_dir.join(format!(
            ".prepare-{}-{}-{}",
            safe_version,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700).create(&path)
        };
        #[cfg(not(unix))]
        let result = std::fs::create_dir(&path);

        match result {
            Ok(()) => return Ok(path),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("创建临时版本目录失败: {err}")),
        }
    }
    Err("无法分配唯一的临时版本目录".to_string())
}

fn rename_release_dir_noreplace(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| "临时版本目录包含无效字符".to_string())?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| "版本目录包含无效字符".to_string())?;

        #[cfg(target_os = "linux")]
        // SAFETY: both paths are valid NUL-terminated strings and remain alive for the call.
        let status = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        // SAFETY: both paths are valid NUL-terminated strings and remain alive for the call.
        let status =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };

        if status == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err("版本目录已存在，拒绝覆盖以保留回滚完整性".to_string());
        }
        Err(format!("安装版本目录失败: {error}"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, destination);
        Err("当前平台不支持安全的原子版本安装".to_string())
    }
}

fn sync_release_tree(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|err| format!("读取待安装版本元数据失败: {err}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("待安装版本包含符号链接: {}", path.display()));
    }
    if metadata.is_file() {
        return std::fs::File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|err| format!("同步待安装版本文件失败: {err}"));
    }
    if !metadata.is_dir() {
        return Err(format!("待安装版本包含特殊文件: {}", path.display()));
    }
    for entry in std::fs::read_dir(path).map_err(|err| format!("读取待安装版本目录失败: {err}"))?
    {
        let entry = entry.map_err(|err| format!("读取待安装版本条目失败: {err}"))?;
        sync_release_tree(&entry.path())?;
    }
    sync_update_directory(path).map_err(|err| format!("同步待安装版本目录失败: {err}"))
}

fn sync_update_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

fn unpack_release_archive(tarball_bytes: &[u8], staging_dir: &Path) -> Result<(), String> {
    unpack_release_archive_with_limits(
        tarball_bytes,
        staging_dir,
        MAX_RELEASE_ARCHIVE_ENTRIES,
        MAX_EXTRACTED_RELEASE_BYTES,
    )
}

fn unpack_release_archive_with_limits(
    tarball_bytes: &[u8],
    staging_dir: &Path,
    max_entries: usize,
    max_extracted_bytes: u64,
) -> Result<(), String> {
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(tarball_bytes));
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|err| format!("读取更新包失败: {err}"))?;
    let mut extracted_bytes = 0u64;
    let mut entry_count = 0usize;
    let mut entry_paths = HashSet::new();

    for entry in entries {
        entry_count = entry_count.saturating_add(1);
        if entry_count > max_entries {
            return Err(format!("更新包条目过多: 最大允许 {max_entries} 个条目"));
        }
        let mut entry = entry.map_err(|err| format!("读取更新包条目失败: {err}"))?;
        let path = entry
            .path()
            .map_err(|err| format!("读取更新包路径失败: {err}"))?
            .to_path_buf();
        let normalized_path = validate_archive_entry_path(&path)?;
        if !entry_paths.insert(normalized_path) {
            return Err(format!("更新包包含重复路径: {}", path.display()));
        }

        let entry_type = entry.header().entry_type();
        if entry_type.is_file() {
            let size = entry
                .header()
                .size()
                .map_err(|err| format!("读取更新包文件大小失败: {err}"))?;
            extracted_bytes = extracted_bytes.saturating_add(size);
            if extracted_bytes > max_extracted_bytes {
                return Err(format!(
                    "更新包解压后过大: 最大允许 {max_extracted_bytes} bytes"
                ));
            }
        } else if !entry_type.is_dir() {
            return Err(format!("更新包包含不支持的条目: {}", path.display()));
        }
        let mode = entry
            .header()
            .mode()
            .map_err(|err| format!("读取更新包权限失败: {err}"))?;
        if mode & 0o7022 != 0 {
            return Err(format!("更新包包含不安全权限: {}", path.display()));
        }

        let unpacked = entry
            .unpack_in(staging_dir)
            .map_err(|err| format!("解压更新包失败: {err}"))?;
        if !unpacked {
            return Err(format!("更新包包含非法路径: {}", path.display()));
        }
    }

    Ok(())
}

fn validate_archive_entry_path(path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                depth = depth.saturating_add(1);
                if depth > MAX_RELEASE_ARCHIVE_PATH_DEPTH {
                    return Err(format!("更新包路径层级过深: {}", path.display()));
                }
                normalized.push(value);
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(format!("更新包包含非法路径: {}", path.display()));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        Err("更新包包含空路径".to_string())
    } else {
        Ok(normalized)
    }
}

fn find_release_payload_dir(staging_dir: &Path) -> Result<PathBuf, String> {
    if looks_like_release_payload(staging_dir) {
        return Ok(staging_dir.to_path_buf());
    }

    let mut candidates = Vec::new();
    let mut top_level_entries = 0usize;
    let entries =
        std::fs::read_dir(staging_dir).map_err(|err| format!("读取更新包目录失败: {err}"))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取更新包条目失败: {err}"))?;
        top_level_entries = top_level_entries.saturating_add(1);
        let path = entry.path();
        if path.is_dir() && looks_like_release_payload(&path) {
            candidates.push(path);
        }
    }

    match candidates.len() {
        1 if top_level_entries == 1 => Ok(candidates.remove(0)),
        1 => Err("更新包必须只包含一个顶层版本目录".to_string()),
        0 => Err(
            "\u{66f4}\u{65b0}\u{5305}\u{4e2d}\u{672a}\u{627e}\u{5230} bin/aether-gateway"
                .to_string(),
        ),
        _ => Err("更新包中包含多个可安装目录，无法确定目标版本".to_string()),
    }
}

fn looks_like_release_payload(path: &Path) -> bool {
    is_nonsymlink_regular_file(&path.join("bin/aether-gateway"))
        && is_nonsymlink_directory(&path.join("frontend"))
}

fn validate_release_payload_dir(path: &Path) -> Result<(), String> {
    let mut entry_count = 0usize;
    validate_release_tree_entry(path, 0, &mut entry_count)?;

    if !is_nonsymlink_regular_file(&path.join("bin/aether-gateway")) {
        return Err(
            "\u{66f4}\u{65b0}\u{5305}\u{4e2d}\u{672a}\u{627e}\u{5230} bin/aether-gateway"
                .to_string(),
        );
    }
    if !is_nonsymlink_regular_file(&path.join("frontend/index.html")) {
        return Err("更新包中未找到 frontend/index.html".to_string());
    }
    Ok(())
}

fn is_nonsymlink_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
}

fn is_nonsymlink_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
}

fn validate_release_tree_entry(
    path: &Path,
    depth: usize,
    entry_count: &mut usize,
) -> Result<(), String> {
    if depth > MAX_RELEASE_ARCHIVE_PATH_DEPTH {
        return Err(format!("版本目录路径层级过深: {}", path.display()));
    }
    *entry_count = entry_count.saturating_add(1);
    if *entry_count > MAX_RELEASE_ARCHIVE_ENTRIES.saturating_add(1) {
        return Err(format!(
            "版本目录条目过多: 最大允许 {MAX_RELEASE_ARCHIVE_ENTRIES} 个条目"
        ));
    }

    let metadata =
        std::fs::symlink_metadata(path).map_err(|err| format!("读取版本目录属性失败: {err}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("版本目录包含符号链接: {}", path.display()));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(format!("版本目录包含特殊文件: {}", path.display()));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // SAFETY: geteuid has no preconditions and retains no pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid {
            return Err(format!(
                "版本目录包含非当前用户所有的条目: {}",
                path.display()
            ));
        }
        if metadata.mode() & 0o7022 != 0 {
            return Err(format!("版本目录包含不安全权限: {}", path.display()));
        }
        if metadata.is_file() && metadata.nlink() != 1 {
            return Err(format!("版本目录包含硬链接文件: {}", path.display()));
        }
    }

    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|err| format!("读取版本目录失败: {err}"))?
        {
            let entry = entry.map_err(|err| format!("读取版本目录条目失败: {err}"))?;
            validate_release_tree_entry(&entry.path(), depth.saturating_add(1), entry_count)?;
        }
    }
    Ok(())
}

fn ensure_release_binary_permissions(binary_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(binary_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("设置更新程序权限失败: {err}"))?;
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            std::fs::remove_dir_all(path)
        }
        Ok(_) => std::fs::remove_file(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

pub(crate) async fn start_admin_system_update_task(
    version: Option<String>,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    if !self_update_supported() {
        return Ok(Err(self_update_rejection_response()));
    }

    let version = match version.or_else(get_prepared_version) {
        Some(v) => v,
        None => {
            return Ok(Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "\u{672a}\u{6307}\u{5b9a}\u{7248}\u{672c}\u{4e14}\u{6ca1}\u{6709}\u{5df2}\u{51c6}\u{5907}\u{7684}\u{66f4}\u{65b0}" }),
            )));
        }
    };

    let release_dir = match release_dir_for_version(&version) {
        Ok(dir) => dir,
        Err(_) => {
            return Ok(Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "版本号无效" }),
            )));
        }
    };
    if !release_dir.join("bin/aether-gateway").is_file() {
        return Ok(Err((
            http::StatusCode::PRECONDITION_REQUIRED,
            json!({ "detail": "指定版本尚未准备好，请先执行 prepare-update" }),
        )));
    }

    let Some(guard) = SystemUpdateGuard::try_acquire() else {
        return Ok(Err(update_already_running_response()));
    };

    if let Err(err) = save_previous_release() {
        set_update_task_failed(err.clone());
        append_update_history("apply", false, Some(&err), None);
        return Ok(Err((
            http::StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "detail": "无法安全保存回滚状态，已取消更新" }),
        )));
    }
    set_update_task_phase("restarting");

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        match switch_current_symlink(&version) {
            Ok(_) => {
                append_update_history(
                    "apply",
                    true,
                    None,
                    Some(&format!(
                        "\u{5df2}\u{5207}\u{6362}\u{5230}\u{7248}\u{672c} {version}"
                    )),
                );
                tracing::info!(version = %version, "update applied, exiting for restart");
                request_process_restart();
            }
            Err(err) => {
                let prev_path = aether_base_dir().join(PREVIOUS_RELEASE_FILENAME);
                if let Err(cleanup_err) = remove_update_metadata_file(&prev_path) {
                    tracing::warn!(
                        error = %safe_system_update_error(&cleanup_err),
                        "failed to clear rollback metadata after update failure"
                    );
                }
                let safe_error = safe_system_update_error(&err);
                tracing::error!(error = %safe_error, "admin system update apply failed");
                append_update_history("apply", false, Some(&err), None);
                set_update_task_failed(err);
            }
        }
        drop(guard);
    });

    Ok(Ok(json!({
        "message": "\u{6b63}\u{5728}\u{5207}\u{6362}\u{7248}\u{672c}\u{5e76}\u{91cd}\u{542f}\u{ff0c}\u{670d}\u{52a1}\u{4f1a}\u{77ed}\u{6682}\u{4e0d}\u{53ef}\u{7528}",
        "started": true,
        "need_restart": true,
    })))
}

fn save_previous_release() -> Result<(), String> {
    save_previous_release_at(&aether_base_dir())
}

fn save_previous_release_at(base_dir: &Path) -> Result<(), String> {
    let name =
        current_release_name_at(base_dir).ok_or_else(|| "当前版本符号链接不安全".to_string())?;
    let prev_path = base_dir.join(PREVIOUS_RELEASE_FILENAME);
    write_update_metadata_atomic(&prev_path, name.as_bytes())
}

fn switch_current_symlink(version: &str) -> Result<(), String> {
    switch_current_symlink_at(&aether_base_dir(), version)
}

fn switch_current_symlink_at(base_dir: &Path, version: &str) -> Result<(), String> {
    let safe_version = safe_release_name(version)?;
    let target = base_dir.join("releases").join(&safe_version);
    let target_metadata = std::fs::symlink_metadata(&target).map_err(|err| {
        format!(
            "\u{7248}\u{672c}\u{76ee}\u{5f55}\u{4e0d}\u{5b58}\u{5728}: {} ({err})",
            target.display()
        )
    })?;
    if target_metadata.file_type().is_symlink() || !target_metadata.is_dir() {
        return Err(format!(
            "\u{7248}\u{672c}\u{76ee}\u{5f55}\u{4e0d}\u{5b58}\u{5728}: {}",
            target.display()
        ));
    }
    validate_release_payload_dir(&target)?;

    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;

        let canonical_base =
            std::fs::canonicalize(base_dir).map_err(|err| format!("解析安装目录失败: {err}"))?;
        let parent = open_real_update_directory(&canonical_base)?;
        let current_name =
            unix_update_path_component(std::ffi::OsStr::new("current"), "当前版本符号链接名")?;
        let Some(current_stat) = unix_update_file_stat_at(&parent, &current_name)? else {
            return Err("当前版本符号链接不存在".to_string());
        };
        // SAFETY: geteuid has no preconditions and retains no pointers.
        let effective_uid = unsafe { libc::geteuid() };
        if current_stat.st_mode & libc::S_IFMT != libc::S_IFLNK
            || current_stat.st_uid != effective_uid
        {
            return Err("当前版本入口不是受当前进程管理的符号链接".to_string());
        }

        let temp_file_name = std::ffi::OsString::from(format!(
            ".current-{}-{}.new",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let temp_name = unix_update_path_component(&temp_file_name, "临时版本符号链接名")?;
        let relative_target = std::ffi::CString::new(format!("releases/{safe_version}"))
            .map_err(|_| "版本符号链接目标包含 NUL 字节".to_string())?;

        // SAFETY: parent is live and both target and temp_name are NUL-terminated for the call.
        let symlink_status = unsafe {
            libc::symlinkat(
                relative_target.as_ptr(),
                parent.as_raw_fd(),
                temp_name.as_ptr(),
            )
        };
        if symlink_status != 0 {
            return Err(format!(
                "创建唯一临时版本符号链接失败: {}",
                std::io::Error::last_os_error()
            ));
        }

        // SAFETY: parent is live and both names are NUL-terminated for the call. renameat
        // atomically replaces the current directory entry without following either symlink.
        let rename_status = unsafe {
            libc::renameat(
                parent.as_raw_fd(),
                temp_name.as_ptr(),
                parent.as_raw_fd(),
                current_name.as_ptr(),
            )
        };
        if rename_status != 0 {
            let error = std::io::Error::last_os_error();
            let _ = unix_update_unlink_at(&parent, &temp_name);
            return Err(format!("原子切换版本符号链接失败: {error}"));
        }
        parent
            .sync_all()
            .map_err(|err| format!("同步版本入口目录失败: {err}"))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (base_dir, safe_version);
        Err("当前平台不支持安全的原子版本切换".to_string())
    }
}

pub(crate) async fn start_admin_system_rollback_task(
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    if !self_update_supported() {
        return Ok(Err(self_update_rejection_response()));
    }

    let Some(previous) = find_rollback_target() else {
        return Ok(Err((
            http::StatusCode::PRECONDITION_REQUIRED,
            json!({ "detail": "\u{6ca1}\u{6709}\u{53ef}\u{56de}\u{6eda}\u{7684}\u{7248}\u{672c}" }),
        )));
    };

    let Some(guard) = SystemUpdateGuard::try_acquire() else {
        return Ok(Err(update_already_running_response()));
    };

    set_update_task_phase("rolling_back");
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        match switch_current_symlink(&previous) {
            Ok(_) => {
                let prev_path = aether_base_dir().join(PREVIOUS_RELEASE_FILENAME);
                if let Err(err) = remove_update_metadata_file(&prev_path) {
                    tracing::warn!(error = %safe_system_update_error(&err), "failed to clear rollback metadata");
                }

                append_update_history(
                    "rollback",
                    true,
                    None,
                    Some(&format!(
                        "\u{5df2}\u{56de}\u{6eda}\u{5230}\u{7248}\u{672c} {previous}"
                    )),
                );
                tracing::info!(version = %previous, "rollback applied, exiting for restart");
                request_process_restart();
            }
            Err(err) => {
                let safe_error = safe_system_update_error(&err);
                tracing::error!(error = %safe_error, "admin system rollback failed");
                append_update_history("rollback", false, Some(&err), None);
                set_update_task_failed(err);
            }
        }
        drop(guard);
    });

    Ok(Ok(json!({
        "message": "\u{56de}\u{6eda}\u{5df2}\u{542f}\u{52a8}\u{ff0c}\u{670d}\u{52a1}\u{4f1a}\u{77ed}\u{6682}\u{4e0d}\u{53ef}\u{7528}",
        "started": true,
        "need_restart": true,
    })))
}

fn request_process_restart() -> ! {
    let _ = aether_runtime::shutdown_logging(std::time::Duration::from_secs(2));
    std::process::exit(RESTART_EXIT_CODE);
}

fn update_already_running_response() -> (http::StatusCode, serde_json::Value) {
    (
        http::StatusCode::CONFLICT,
        json!({ "detail": "\u{5df2}\u{6709}\u{4e00}\u{952e}\u{66f4}\u{65b0}\u{4efb}\u{52a1}\u{6b63}\u{5728}\u{6267}\u{884c}" }),
    )
}

fn self_update_rejection_response() -> (http::StatusCode, serde_json::Value) {
    (
        http::StatusCode::PRECONDITION_REQUIRED,
        json!({ "detail": current_self_update_blocker() }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;

    fn temp_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aether-update-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn write_release_payload(root: &Path) {
        std::fs::create_dir_all(root.join("bin")).expect("bin dir should be created");
        std::fs::create_dir_all(root.join("frontend")).expect("frontend dir should be created");
        std::fs::write(root.join("bin/aether-gateway"), b"test-binary")
            .expect("binary should be written");
        std::fs::write(root.join("frontend/index.html"), b"<html></html>")
            .expect("frontend index should be written");
    }

    #[test]
    fn reading_update_history_rewrites_legacy_sensitive_fields() {
        let dir = temp_test_dir("history-read-redaction");
        let path = dir.join(UPDATE_HISTORY_FILENAME);
        std::fs::create_dir_all(&dir).expect("history directory should be created");
        let legacy = json!([{
            "timestamp": "Bearer timestamp-secret",
            "operation": "prepare?token=operation-secret",
            "success": false,
            "error": "download failed for https://user:password@internal.test/file?token=query-secret; Authorization: Bearer error-secret",
            "output_tail": "installed /opt/private; access_token=output-secret"
        }]);
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&legacy).expect("legacy history should serialize"),
        )
        .expect("legacy history should be written");

        let entries = read_update_history_at_path(&path);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, "1970-01-01T00:00:00Z");
        assert_eq!(entries[0].operation, "unknown");
        assert_eq!(
            entries[0].error.as_deref(),
            Some("Update package download failed")
        );
        assert_eq!(
            entries[0].output_tail.as_deref(),
            Some("System update step completed")
        );

        let persisted = std::fs::read_to_string(&path).expect("history should remain readable");
        for secret in [
            "timestamp-secret",
            "operation-secret",
            "user:password",
            "query-secret",
            "error-secret",
            "/opt/private",
            "output-secret",
        ] {
            assert!(
                !persisted.contains(secret),
                "persisted history leaked {secret}"
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn appending_update_history_sanitizes_existing_entries_before_write() {
        let dir = temp_test_dir("history-append-redaction");
        let path = dir.join(UPDATE_HISTORY_FILENAME);
        std::fs::create_dir_all(&dir).expect("history directory should be created");
        let legacy = json!([{
            "timestamp": "2026-08-27T00:00:00Z",
            "operation": "apply",
            "success": false,
            "error": "Authorization: Bearer legacy-secret",
            "output_tail": "legacy output token=old-secret"
        }]);
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&legacy).expect("legacy history should serialize"),
        )
        .expect("legacy history should be written");

        append_update_history_at_path(
            &path,
            "rollback",
            false,
            Some("failed with Bearer new-secret"),
            Some("private output new-output-secret"),
        );

        let persisted = std::fs::read_to_string(&path).expect("history should remain readable");
        assert!(!persisted.contains("legacy-secret"));
        assert!(!persisted.contains("old-secret"));
        assert!(!persisted.contains("new-secret"));
        assert!(!persisted.contains("new-output-secret"));
        let entries: Vec<UpdateHistoryEntry> =
            serde_json::from_str(&persisted).expect("sanitized history should parse");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].error.as_deref(), Some("System update failed"));
        assert_eq!(
            entries[1].output_tail.as_deref(),
            Some("System rollback applied")
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn update_metadata_atomic_write_is_private_and_rejects_symlink_destination() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = temp_test_dir("metadata-atomic");
        std::fs::create_dir_all(&dir).expect("metadata directory should be created");
        let path = dir.join(UPDATE_HISTORY_FILENAME);

        write_update_metadata_atomic(&path, b"first").expect("metadata should be written");
        write_update_metadata_atomic(&path, b"second").expect("metadata should be replaced");
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0,
            "update metadata must remain private"
        );

        std::fs::remove_file(&path).unwrap();
        let victim = dir.join("victim");
        std::fs::write(&victim, b"known-good").unwrap();
        symlink(&victim, &path).expect("metadata symlink should be created");

        let err = write_update_metadata_atomic(&path, b"attacker-controlled")
            .expect_err("symlink destination must be rejected");

        assert!(err.contains("安全的普通文件"));
        assert!(remove_update_metadata_file(&path).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "rejected metadata symlink must not be followed or replaced"
        );
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".aether-update-metadata-")
            }),
            "atomic write must not leave temporary files"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn update_history_does_not_read_or_rewrite_symlink_target() {
        use std::os::unix::fs::symlink;

        let dir = temp_test_dir("history-symlink");
        std::fs::create_dir_all(&dir).expect("history directory should be created");
        let victim = dir.join("victim.json");
        let victim_contents = br#"[{"timestamp":"2026-01-01T00:00:00Z","operation":"apply","success":true,"error":null,"output_tail":null}]"#;
        std::fs::write(&victim, victim_contents).unwrap();
        let path = dir.join(UPDATE_HISTORY_FILENAME);
        symlink(&victim, &path).expect("history symlink should be created");

        assert!(read_update_history_at_path(&path).is_empty());
        assert_eq!(std::fs::read(&victim).unwrap(), victim_contents);
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn previous_release_metadata_is_atomic_and_rejects_symlink_input() {
        use std::os::unix::fs::symlink;

        let base = temp_test_dir("previous-release");
        let release = base.join("releases/v1.2.3");
        write_release_payload(&release);
        symlink(&release, base.join("current")).expect("current link should be created");

        save_previous_release_at(&base).expect("previous release should be persisted");
        assert_eq!(find_rollback_target_at(&base).as_deref(), Some("v1.2.3"));

        let previous = base.join(PREVIOUS_RELEASE_FILENAME);
        std::fs::remove_file(&previous).unwrap();
        let victim = base.join("victim");
        std::fs::write(&victim, b"v1.2.3").unwrap();
        symlink(&victim, &previous).expect("previous-release symlink should be created");

        assert!(find_rollback_target_at(&base).is_none());
        assert!(save_previous_release_at(&base).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"v1.2.3");
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn previous_release_rejects_current_link_outside_managed_releases() {
        use std::os::unix::fs::symlink;

        let base = temp_test_dir("outside-current");
        std::fs::create_dir_all(base.join("releases")).unwrap();
        let outside = temp_test_dir("outside-current-target");
        write_release_payload(&outside);
        symlink(&outside, base.join("current")).expect("outside current link should be created");

        assert!(current_release_name_at(&base).is_none());
        assert!(save_previous_release_at(&base).is_err());
        assert!(!base.join(PREVIOUS_RELEASE_FILENAME).exists());
        std::fs::remove_dir_all(base).ok();
        std::fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn update_strategy_defaults_to_self_only_for_release_builds() {
        assert_eq!(
            UpdateStrategy::from_env_value(None, true),
            UpdateStrategy::SelfManaged
        );
        assert_eq!(
            UpdateStrategy::from_env_value(None, false),
            UpdateStrategy::Manual
        );
    }

    #[test]
    fn update_strategy_parses_docker_as_non_self_update() {
        assert_eq!(
            UpdateStrategy::from_env_value(Some("docker"), true),
            UpdateStrategy::Docker
        );
        assert_eq!(
            UpdateStrategy::from_env_value(Some("compose"), true),
            UpdateStrategy::Docker
        );
        assert_eq!(
            UpdateStrategy::from_env_value(Some("unknown"), true),
            UpdateStrategy::Manual
        );
    }

    #[test]
    fn deployment_topology_defaults_to_single_node() {
        assert_eq!(
            DeploymentTopology::from_env_value(None),
            DeploymentTopology::SingleNode
        );
        assert_eq!(
            DeploymentTopology::from_env_value(Some("multi-node")),
            DeploymentTopology::MultiNode
        );
    }

    #[test]
    fn multi_node_topology_disables_self_update() {
        assert!(self_update_supported_for(
            true,
            UpdateStrategy::SelfManaged,
            DeploymentTopology::SingleNode,
        ));
        assert!(!self_update_supported_for(
            true,
            UpdateStrategy::SelfManaged,
            DeploymentTopology::MultiNode,
        ));
    }

    #[test]
    fn update_capability_omits_internal_paths_and_dynamic_commands() {
        let payload = build_admin_system_update_capability_payload();

        for field in ["install_root", "base_dir", "data_dir", "logs_dir"] {
            assert!(payload.get(field).is_none(), "capability exposed {field}");
        }
        if let Some(command) = payload
            .get("docker_update_command")
            .and_then(serde_json::Value::as_str)
        {
            assert_eq!(command, "./update.sh");
        }
    }

    #[test]
    fn update_finds_nested_release_payload_dir() {
        let staging = temp_test_dir("nested");
        let bundle = staging.join("aether-v1.2.3-linux-amd64");
        write_release_payload(&bundle);

        let found = find_release_payload_dir(&staging).expect("payload dir should be found");

        assert_eq!(found, bundle);
        std::fs::remove_dir_all(staging).ok();
    }

    #[test]
    fn update_rejects_nested_payload_with_extra_top_level_entries() {
        let staging = temp_test_dir("nested-extra");
        let bundle = staging.join("aether-v1.2.3-linux-amd64");
        write_release_payload(&bundle);
        std::fs::write(staging.join("unexpected"), b"extra")
            .expect("extra entry should be written");

        let err = find_release_payload_dir(&staging)
            .expect_err("extra top-level entries must be rejected");

        assert!(err.contains("一个顶层版本目录"));
        std::fs::remove_dir_all(staging).ok();
    }

    #[cfg(unix)]
    #[test]
    fn update_revalidates_release_tree_before_activation() {
        use std::os::unix::fs::symlink;

        let release = temp_test_dir("payload-revalidation");
        write_release_payload(&release);
        let victim = release.join("victim");
        std::fs::write(&victim, b"outside-index").unwrap();
        std::fs::remove_file(release.join("frontend/index.html")).unwrap();
        symlink(&victim, release.join("frontend/index.html")).unwrap();

        let err = validate_release_payload_dir(&release)
            .expect_err("post-extraction symlink must be rejected");
        assert!(err.contains("符号链接"));

        std::fs::remove_file(release.join("frontend/index.html")).unwrap();
        std::fs::hard_link(&victim, release.join("frontend/index.html")).unwrap();
        let err = validate_release_payload_dir(&release)
            .expect_err("post-extraction hard link must be rejected");
        assert!(err.contains("硬链接"));
        std::fs::remove_dir_all(release).ok();
    }

    #[cfg(unix)]
    #[test]
    fn update_switch_uses_unique_relative_symlink_without_deleting_predictable_paths() {
        use std::os::unix::fs::symlink;

        let base = temp_test_dir("atomic-current-switch");
        write_release_payload(&base.join("releases/v1.0.0"));
        write_release_payload(&base.join("releases/v2.0.0"));
        symlink("releases/v1.0.0", base.join("current")).unwrap();
        let predictable = base.join("current.new");
        std::fs::create_dir(&predictable).unwrap();
        std::fs::write(predictable.join("keep"), b"known-good").unwrap();

        switch_current_symlink_at(&base, "v2.0.0").expect("version switch should succeed");

        assert_eq!(
            std::fs::read_link(base.join("current")).unwrap(),
            PathBuf::from("releases/v2.0.0")
        );
        assert_eq!(
            std::fs::read(predictable.join("keep")).unwrap(),
            b"known-good"
        );
        assert!(std::fs::read_dir(&base).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".current-")
        }));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn update_finds_flat_release_payload_dir() {
        let staging = temp_test_dir("flat");
        write_release_payload(&staging);

        let found = find_release_payload_dir(&staging).expect("payload dir should be found");

        assert_eq!(found, staging);
        std::fs::remove_dir_all(found).ok();
    }

    #[test]
    fn update_accepts_release_workflow_archive_layout() {
        let fixture = temp_test_dir("workflow-layout-source");
        let payload = fixture.join("payload");
        write_release_payload(&payload);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            builder
                .append_dir_all("aether-v1.2.3-linux-amd64", &payload)
                .expect("workflow-style bundle should be appended");
            builder.finish().expect("tar builder should finish");
        }
        let tarball = encoder.finish().expect("gzip encoder should finish");
        let staging = temp_test_dir("workflow-layout-destination");
        std::fs::create_dir_all(&staging).expect("staging should be created");

        unpack_release_archive(&tarball, &staging)
            .expect("workflow-style release archive should unpack");
        let found = find_release_payload_dir(&staging)
            .expect("workflow-style release payload should be found");
        validate_release_payload_dir(&found)
            .expect("workflow-style release payload should validate");

        assert_eq!(
            found.file_name().and_then(|name| name.to_str()),
            Some("aether-v1.2.3-linux-amd64")
        );
        std::fs::remove_dir_all(fixture).ok();
        std::fs::remove_dir_all(staging).ok();
    }

    #[test]
    fn update_rejects_unsafe_release_names() {
        assert!(safe_release_name("v1.2.3").is_ok());
        assert!(safe_release_name("../v1.2.3").is_err());
        assert!(safe_release_name("v1.2.3/linux").is_err());
        assert!(safe_release_name("").is_err());
    }

    #[test]
    fn update_validates_download_urls() {
        assert!(validate_update_download_url(
            "https://github.com/fawney19/Aether/releases/download/v1/aether.tar.gz"
        )
        .is_ok());
        assert!(validate_update_download_url(
            "https://objects.githubusercontent.com/github-production-release-asset/test"
        )
        .is_ok());
        assert!(validate_update_download_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/test"
        )
        .is_ok());
        assert!(validate_update_download_url(
            "http://github.com/fawney19/Aether/releases/download/v1/aether.tar.gz"
        )
        .is_err());
        assert!(validate_update_download_url(
            "https://user@github.com/fawney19/Aether/releases/download/v1/aether.tar.gz"
        )
        .is_err());
        assert!(
            validate_update_download_url("https://github.com.evil.test/aether.tar.gz").is_err()
        );
        assert!(validate_update_download_url("https://example.com/aether.tar.gz").is_err());
    }

    #[test]
    fn update_binds_tarball_and_checksum_to_official_same_version_release() {
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "amd64"
        };
        let tarball = format!(
            "https://github.com/fawney19/Aether/releases/download/v1.2.3/aether-v1.2.3-{platform}-{arch}.tar.gz"
        );
        let checksums = "https://github.com/fawney19/Aether/releases/download/v1.2.3/SHA256SUMS";

        validate_update_release_urls("v1.2.3", &tarball, checksums)
            .expect("matching official release assets should pass");

        for (bad_tarball, bad_checksums) in [
            (
                tarball.replace("fawney19/Aether", "attacker/project"),
                checksums.to_string(),
            ),
            (tarball.clone(), checksums.replace("/v1.2.3/", "/v1.2.2/")),
            (
                tarball.replace(&format!("-{arch}.tar.gz"), "-wrong.tar.gz"),
                checksums.to_string(),
            ),
            (format!("{tarball}?token=unexpected"), checksums.to_string()),
        ] {
            assert!(
                validate_update_release_urls("v1.2.3", &bad_tarball, &bad_checksums).is_err(),
                "mismatched update assets must be rejected"
            );
        }
    }

    #[test]
    fn update_url_validation_and_error_projection_do_not_echo_sensitive_details() {
        let rejected = validate_update_download_url(
            "https://user:password@github.com/release.tar.gz?token=query-secret",
        )
        .expect_err("credential-bearing update URL must be rejected");
        assert!(!rejected.contains("user:password"));
        assert!(!rejected.contains("query-secret"));

        let projected = safe_system_update_error(
            "request failed for https://user:password@internal.test?q=secret; Authorization: Bearer upstream-secret",
        );
        assert_eq!(projected, "Update package download failed");
        assert!(!projected.contains("upstream-secret"));
        assert!(!projected.contains("user:password"));

        assert_eq!(
            safe_system_update_error(
                "download failed: https://user:password@internal.test?q=secret"
            ),
            "Update package download failed"
        );
        assert_eq!(
            safe_system_update_error("version directory missing: /opt/aether/releases/secret"),
            "Update installation failed"
        );
    }

    #[test]
    fn update_rejects_archive_path_traversal() {
        assert!(validate_archive_entry_path(Path::new("bundle/bin/aether-gateway")).is_ok());
        assert!(validate_archive_entry_path(Path::new("./bundle/bin/aether-gateway")).is_err());
        assert!(validate_archive_entry_path(Path::new("../escape")).is_err());
        assert!(validate_archive_entry_path(Path::new("/tmp/escape")).is_err());
    }

    #[test]
    fn update_rejects_archive_symlinks() {
        let staging = temp_test_dir("symlink");
        std::fs::create_dir_all(&staging).expect("staging dir should be created");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header
                .set_link_name("/bin/sh")
                .expect("link name should be set");
            header.set_cksum();
            builder
                .append_data(&mut header, "bundle/bin/aether-gateway", std::io::empty())
                .expect("symlink entry should be appended");
            builder.finish().expect("tar builder should finish");
        }
        let tarball = encoder.finish().expect("gzip encoder should finish");

        let err = unpack_release_archive(&tarball, &staging).expect_err("archive should fail");

        assert!(err.contains("不支持的条目"));
        std::fs::remove_dir_all(staging).ok();
    }

    #[test]
    fn update_rejects_duplicate_archive_paths() {
        let staging = temp_test_dir("duplicate-path");
        std::fs::create_dir_all(&staging).expect("staging dir should be created");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            for contents in [b"first".as_slice(), b"second".as_slice()] {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(contents.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, "bundle/frontend/index.html", contents)
                    .expect("duplicate fixture entry should be appended");
            }
            builder.finish().expect("tar builder should finish");
        }
        let tarball = encoder.finish().expect("gzip encoder should finish");

        let err = unpack_release_archive(&tarball, &staging)
            .expect_err("duplicate archive path should fail");

        assert!(err.contains("重复路径"));
        std::fs::remove_dir_all(staging).ok();
    }

    #[test]
    fn update_enforces_archive_entry_limit() {
        let staging = temp_test_dir("entry-limit");
        std::fs::create_dir_all(&staging).expect("staging dir should be created");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            for (path, contents) in [("bundle/one", b"one"), ("bundle/two", b"two")] {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(contents.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, contents.as_slice())
                    .expect("entry-limit fixture should be appended");
            }
            builder.finish().expect("tar builder should finish");
        }
        let tarball = encoder.finish().expect("gzip encoder should finish");

        let err = unpack_release_archive_with_limits(&tarball, &staging, 1, u64::MAX)
            .expect_err("archive entry limit should be enforced");

        assert!(err.contains("条目过多"));
        std::fs::remove_dir_all(staging).ok();
    }

    #[test]
    fn update_rejects_group_or_world_writable_archive_members() {
        let staging = temp_test_dir("writable-member");
        std::fs::create_dir_all(&staging).expect("staging dir should be created");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            let contents = b"test-binary";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(contents.len() as u64);
            header.set_mode(0o777);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "bundle/bin/aether-gateway",
                    contents.as_slice(),
                )
                .expect("writable fixture entry should be appended");
            builder.finish().expect("tar builder should finish");
        }
        let tarball = encoder.finish().expect("gzip encoder should finish");

        let err = unpack_release_archive(&tarball, &staging)
            .expect_err("writable archive member should fail");

        assert!(err.contains("不安全权限"));
        std::fs::remove_dir_all(staging).ok();
    }

    #[test]
    fn update_preserves_existing_release_destination() {
        let dir = temp_test_dir("existing-release");
        let source = dir.join("prepared-v1.2.3");
        let release = dir.join("releases/v1.2.3");
        std::fs::create_dir_all(&source).expect("prepared release should be created");
        std::fs::create_dir_all(&release).expect("existing release should be created");
        let sentinel = release.join("keep");
        std::fs::write(&sentinel, b"known-good").expect("sentinel should be written");

        let err = rename_release_dir_noreplace(&source, &release)
            .expect_err("existing release must not be replaceable");

        assert!(err.contains("拒绝覆盖"));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"known-good");
        assert!(source.is_dir(), "failed install must retain its source");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn update_preserves_existing_empty_release_destination() {
        let dir = temp_test_dir("existing-empty-release");
        let source = dir.join("prepared-v1.2.3");
        let release = dir.join("releases/v1.2.3");
        std::fs::create_dir_all(&source).expect("prepared release should be created");
        std::fs::create_dir_all(&release).expect("existing release should be created");

        let err = rename_release_dir_noreplace(&source, &release)
            .expect_err("existing empty release must not be replaceable");

        assert!(err.contains("拒绝覆盖"));
        assert!(
            release.is_dir(),
            "failed install must retain its destination"
        );
        assert!(source.is_dir(), "failed install must retain its source");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn update_atomically_installs_absent_release_destination() {
        let dir = temp_test_dir("new-release");
        let source = dir.join("prepared-v1.2.3");
        let release = dir.join("releases/v1.2.3");
        std::fs::create_dir_all(&source).expect("prepared release should be created");
        std::fs::create_dir_all(release.parent().unwrap())
            .expect("releases parent should be created");
        std::fs::write(source.join("sentinel"), b"prepared")
            .expect("prepared sentinel should be written");

        rename_release_dir_noreplace(&source, &release)
            .expect("absent release should install atomically");

        assert!(!source.exists());
        assert_eq!(
            std::fs::read(release.join("sentinel")).unwrap(),
            b"prepared"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn update_staging_directory_is_unique_private_and_non_destructive() {
        use std::os::unix::fs::PermissionsExt;

        let base = temp_test_dir("private-staging");
        std::fs::create_dir_all(&base).expect("base should be created");
        let legacy = base.join(format!(".prepare-v1.2.3-{}", std::process::id()));
        std::fs::create_dir(&legacy).expect("legacy staging should be created");
        std::fs::write(legacy.join("keep"), b"existing")
            .expect("legacy sentinel should be written");

        let staging =
            create_release_staging_dir(&base, "v1.2.3").expect("unique staging should be created");

        assert_ne!(staging, legacy);
        assert_eq!(
            std::fs::metadata(&staging).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(std::fs::read(legacy.join("keep")).unwrap(), b"existing");
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn self_update_capability_requires_managed_writable_directories() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let base = temp_test_dir("storage-capability");
        let current_release = base.join("releases/v1.2.3");
        write_release_payload(&current_release);
        symlink(&current_release, base.join("current")).expect("current link should be created");
        let base = std::fs::canonicalize(base).expect("base should canonicalize");
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o750)).unwrap();
        std::fs::set_permissions(
            base.join("releases"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();

        assert!(self_update_storage_ready_at(&base));

        std::fs::set_permissions(
            base.join("releases"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(!self_update_storage_ready_at(&base));

        std::fs::set_permissions(
            base.join("releases"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        std::fs::remove_file(base.join("current")).unwrap();
        let outside = temp_test_dir("storage-capability-outside");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, base.join("current")).expect("outside current link should be created");
        assert!(!self_update_storage_ready_at(&base));

        std::fs::remove_dir_all(base).ok();
        std::fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn update_verifies_sha256sum_for_asset_name() {
        let data = b"release-bytes";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let sums = format!("{expected}  aether-v1.2.3-linux-amd64.tar.gz\n");

        verify_sha256(
            data,
            &sums,
            "https://example.test/aether-v1.2.3-linux-amd64.tar.gz",
        )
        .expect("sha256 should match");

        let duplicate = format!(
            "{expected}  aether-v1.2.3-linux-amd64.tar.gz\n{expected} *aether-v1.2.3-linux-amd64.tar.gz\n"
        );
        assert!(verify_sha256(
            data,
            &duplicate,
            "https://example.test/aether-v1.2.3-linux-amd64.tar.gz",
        )
        .is_err());
        assert!(verify_sha256(
            data,
            "not-a-hash  aether-v1.2.3-linux-amd64.tar.gz\n",
            "https://example.test/aether-v1.2.3-linux-amd64.tar.gz",
        )
        .is_err());
    }
}
