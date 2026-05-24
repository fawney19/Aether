use crate::handlers::admin::system::shared::update_client::build_update_http_client;
use crate::GatewayError;
use axum::http;
use futures_util::StreamExt;
use serde_json::json;
use sha2::{Digest, Sha256};
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
        guard.error = Some(error);
    }
}

fn set_update_task_output(output: String) {
    if let Ok(mut guard) = update_task_status_lock().lock() {
        guard.output = Some(output);
    }
}

pub(crate) fn read_update_task_status() -> SystemUpdateTaskStatus {
    update_task_status_lock()
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
        })
}

static PREPARED_VERSION: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();

fn prepared_version_lock() -> &'static Mutex<Option<String>> {
    PREPARED_VERSION.get_or_init(|| Mutex::new(None))
}

fn set_prepared_version(version: String) {
    if let Ok(mut guard) = prepared_version_lock().lock() {
        *guard = Some(version);
    }
}

pub(crate) fn get_prepared_version() -> Option<String> {
    prepared_version_lock().lock().ok()?.clone()
}

const UPDATE_HISTORY_FILENAME: &str = ".aether-update-history.json";
const PREVIOUS_RELEASE_FILENAME: &str = ".aether-previous-release";
const MAX_HISTORY_ENTRIES: usize = 50;
const RESTART_EXIT_CODE: i32 = 75;
const MAX_RELEASE_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SHA256SUMS_DOWNLOAD_BYTES: u64 = 1024 * 1024;
const MAX_EXTRACTED_RELEASE_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_UPDATE_DOWNLOAD_TIMEOUT_SECS: u64 = 600;
const DEFAULT_UPDATE_DOWNLOAD_IDLE_TIMEOUT_SECS: u64 = 30;
const SOURCE_BUILD_UPDATE_BLOCKER: &str = "当前为源码构建，请使用 git pull 后重新编译。";
const DOCKER_UPDATE_BLOCKER: &str =
    "Docker 部署请使用镜像更新：进入 docker-compose.yml 所在目录执行 ./update.sh。";
const MANUAL_UPDATE_BLOCKER: &str =
    "当前部署策略不支持在线自更新，请手动下载 Release 或使用安装脚本更新。";
const MULTI_NODE_UPDATE_BLOCKER: &str =
    "多节点部署不支持在管理后台更新单个节点，请使用镜像滚动更新或外部发布编排。";

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

fn current_symlink_path() -> PathBuf {
    aether_base_dir().join("current")
}

fn current_release_name() -> Option<String> {
    std::fs::read_link(current_symlink_path())
        .ok()?
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

fn update_history_path() -> PathBuf {
    aether_base_dir().join(UPDATE_HISTORY_FILENAME)
}

fn append_update_history(
    operation: &str,
    success: bool,
    error: Option<&str>,
    output: Option<&str>,
) {
    let path = update_history_path();

    let entry = UpdateHistoryEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        operation: operation.to_string(),
        success,
        error: error.map(|s| s.to_string()),
        output_tail: output.map(|s| {
            let lines: Vec<&str> = s.lines().collect();
            let start = lines.len().saturating_sub(20);
            lines[start..].join("\n")
        }),
    };

    let mut entries: Vec<UpdateHistoryEntry> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();

    entries.push(entry);
    if entries.len() > MAX_HISTORY_ENTRIES {
        entries.drain(..entries.len() - MAX_HISTORY_ENTRIES);
    }

    if let Ok(json) = serde_json::to_string_pretty(&entries) {
        let _ = std::fs::write(&path, json);
    }
}

pub(crate) fn read_update_history() -> Vec<UpdateHistoryEntry> {
    let path = update_history_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
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
    )
}

pub(crate) fn current_self_update_blocker() -> &'static str {
    if !is_release_build() {
        return SOURCE_BUILD_UPDATE_BLOCKER;
    }
    if current_deployment_topology() == DeploymentTopology::MultiNode {
        return MULTI_NODE_UPDATE_BLOCKER;
    }

    match current_update_strategy() {
        UpdateStrategy::SelfManaged => "一键更新可用",
        UpdateStrategy::Docker => DOCKER_UPDATE_BLOCKER,
        UpdateStrategy::Manual => MANUAL_UPDATE_BLOCKER,
    }
}

fn update_logs_dir() -> PathBuf {
    std::env::var("AETHER_LOG_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| aether_base_dir().join("logs"))
}

fn docker_update_command() -> String {
    std::env::var("AETHER_DOCKER_UPDATE_COMMAND")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "./update.sh".to_string())
}

pub(crate) fn build_admin_system_update_capability_payload() -> serde_json::Value {
    let build_type = current_build_type();
    let update_strategy = current_update_strategy();
    let deployment_topology = current_deployment_topology();
    let supported =
        self_update_supported_for(is_release_build(), update_strategy, deployment_topology);
    let rollback_available = supported && find_rollback_target().is_some();
    let task_status = read_update_task_status();
    let base_dir = aether_base_dir();
    let docker_command = if update_strategy == UpdateStrategy::Docker {
        Some(docker_update_command())
    } else {
        None
    };
    let data_dir = base_dir.join("data");
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
        "install_root": base_dir.clone(),
        "base_dir": base_dir,
        "data_dir": data_dir,
        "logs_dir": update_logs_dir(),
        "docker_update_command": docker_command,
        "message": if supported {
            "一键更新可用"
        } else {
            current_self_update_blocker()
        },
    })
}

fn find_rollback_target() -> Option<String> {
    let previous_path = aether_base_dir().join(PREVIOUS_RELEASE_FILENAME);
    let previous = std::fs::read_to_string(previous_path).ok()?;
    let previous = previous.trim().to_string();
    if previous.is_empty() {
        return None;
    }
    let target_dir = release_dir_for_version(&previous).ok()?;
    if target_dir.is_dir() {
        Some(previous)
    } else {
        None
    }
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
    let Some(guard) = SystemUpdateGuard::try_acquire() else {
        return Ok(Err(update_already_running_response()));
    };

    set_update_task_phase("preparing");

    tokio::spawn(async move {
        let _guard = guard;
        let total_timeout = update_download_total_timeout();
        let result = match tokio::time::timeout(
            total_timeout,
            download_and_extract_release(&version, &tarball_url, &sha256sums_url),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(format!(
                "下载更新包超时: 超过 {} 秒",
                total_timeout.as_secs()
            )),
        };

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
    let client = build_update_http_client(update_download_total_timeout(), "更新下载")?;

    set_update_task_phase("downloading");
    let tarball_bytes =
        download_update_bytes(&client, tarball_url, MAX_RELEASE_DOWNLOAD_BYTES, "更新包").await?;

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
    .map_err(|err| format!("校验文件不是有效 UTF-8: {err}"))?;

    let tarball_url_owned = tarball_url.to_string();
    let version_owned = version.to_string();
    tokio::task::spawn_blocking(move || {
        set_update_task_phase("verifying");
        verify_sha256(&tarball_bytes, &sha256_text, &tarball_url_owned)?;
        set_update_task_phase("extracting");
        extract_release(&version_owned, &tarball_bytes)
    })
    .await
    .map_err(|err| format!("\u{89e3}\u{538b}\u{4efb}\u{52a1}\u{5f02}\u{5e38}: {err}"))?
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
    .map_err(|err| format!("下载{label}失败: {err}"))?
    .error_for_status()
    .map_err(|err| format!("下载{label}返回错误: {err}"))?;

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
        let chunk = chunk.map_err(|err| format!("读取{label}数据失败: {err}"))?;
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
    let parsed = url::Url::parse(raw_url).map_err(|err| format!("下载 URL 无效: {err}"))?;
    if parsed.scheme() != "https" {
        return Err("下载 URL 必须使用 HTTPS".to_string());
    }
    let Some(host) = parsed.host_str() else {
        return Err("下载 URL 缺少主机名".to_string());
    };
    if host == "github.com"
        || host.ends_with(".github.com")
        || host == "objects.githubusercontent.com"
        || host.ends_with(".objects.githubusercontent.com")
    {
        return Ok(());
    }
    Err(format!("下载 URL 主机不受信任: {host}"))
}

fn verify_sha256(data: &[u8], sums_text: &str, tarball_url: &str) -> Result<(), String> {
    let tarball_filename = tarball_url.rsplit('/').next().ok_or_else(|| {
        "\u{65e0}\u{6cd5}\u{4ece} URL \u{63d0}\u{53d6}\u{6587}\u{4ef6}\u{540d}".to_string()
    })?;

    let expected_hash = sums_text
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            if name == tarball_filename {
                Some(hash.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            format!("SHA256SUMS \u{4e2d}\u{672a}\u{627e}\u{5230} {tarball_filename} \u{7684}\u{6821}\u{9a8c}\u{503c}")
        })?;

    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();
    let actual_hash: String = hash.iter().map(|b| format!("{b:02x}")).collect();

    if actual_hash != expected_hash {
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
    let staging_dir = base_dir.join(format!(".prepare-{}-{}", safe_version, std::process::id()));
    remove_path_if_exists(&staging_dir).map_err(|err| format!("清理临时版本目录失败: {err}"))?;
    std::fs::create_dir_all(&staging_dir).map_err(|err| format!("创建临时版本目录失败: {err}"))?;

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

    remove_path_if_exists(&release_dir).map_err(|err| {
        format!("\u{6e05}\u{7406}\u{65e7}\u{7248}\u{672c}\u{76ee}\u{5f55}\u{5931}\u{8d25}: {err}")
    })?;

    if bundle_dir == staging_dir {
        std::fs::rename(&staging_dir, &release_dir)
            .map_err(|err| format!("安装版本目录失败: {err}"))?;
    } else {
        std::fs::rename(&bundle_dir, &release_dir)
            .map_err(|err| format!("安装版本目录失败: {err}"))?;
        let _ = remove_path_if_exists(&staging_dir);
    }

    ensure_release_binary_permissions(&release_dir.join("bin/aether-gateway"));

    Ok(format!(
        "\u{7248}\u{672c} {} \u{5df2}\u{51c6}\u{5907}\u{5c31}\u{7eea}",
        version
    ))
}

fn unpack_release_archive(tarball_bytes: &[u8], staging_dir: &Path) -> Result<(), String> {
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(tarball_bytes));
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|err| format!("读取更新包失败: {err}"))?;
    let mut extracted_bytes = 0u64;

    for entry in entries {
        let mut entry = entry.map_err(|err| format!("读取更新包条目失败: {err}"))?;
        let path = entry
            .path()
            .map_err(|err| format!("读取更新包路径失败: {err}"))?
            .to_path_buf();
        validate_archive_entry_path(&path)?;

        let entry_type = entry.header().entry_type();
        if entry_type.is_file() {
            let size = entry
                .header()
                .size()
                .map_err(|err| format!("读取更新包文件大小失败: {err}"))?;
            extracted_bytes = extracted_bytes.saturating_add(size);
            if extracted_bytes > MAX_EXTRACTED_RELEASE_BYTES {
                return Err(format!(
                    "更新包解压后过大: 最大允许 {MAX_EXTRACTED_RELEASE_BYTES} bytes"
                ));
            }
        } else if !entry_type.is_dir() {
            return Err(format!("更新包包含不支持的条目: {}", path.display()));
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

fn validate_archive_entry_path(path: &Path) -> Result<(), String> {
    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("更新包包含非法路径: {}", path.display()));
            }
        }
    }
    if has_normal_component {
        Ok(())
    } else {
        Err("更新包包含空路径".to_string())
    }
}

fn find_release_payload_dir(staging_dir: &Path) -> Result<PathBuf, String> {
    if looks_like_release_payload(staging_dir) {
        return Ok(staging_dir.to_path_buf());
    }

    let mut candidates = Vec::new();
    let entries =
        std::fs::read_dir(staging_dir).map_err(|err| format!("读取更新包目录失败: {err}"))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取更新包条目失败: {err}"))?;
        let path = entry.path();
        if path.is_dir() && looks_like_release_payload(&path) {
            candidates.push(path);
        }
    }

    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(
            "\u{66f4}\u{65b0}\u{5305}\u{4e2d}\u{672a}\u{627e}\u{5230} bin/aether-gateway"
                .to_string(),
        ),
        _ => Err("更新包中包含多个可安装目录，无法确定目标版本".to_string()),
    }
}

fn looks_like_release_payload(path: &Path) -> bool {
    path.join("bin/aether-gateway").is_file() && path.join("frontend").is_dir()
}

fn validate_release_payload_dir(path: &Path) -> Result<(), String> {
    if !path.join("bin/aether-gateway").is_file() {
        return Err(
            "\u{66f4}\u{65b0}\u{5305}\u{4e2d}\u{672a}\u{627e}\u{5230} bin/aether-gateway"
                .to_string(),
        );
    }
    if !path.join("frontend/index.html").is_file() {
        return Err("更新包中未找到 frontend/index.html".to_string());
    }
    Ok(())
}

fn ensure_release_binary_permissions(binary_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(binary_path, std::fs::Permissions::from_mode(0o755));
    }
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
        Err(err) => {
            return Ok(Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": err }),
            )));
        }
    };
    if !release_dir.join("bin/aether-gateway").is_file() {
        return Ok(Err((
            http::StatusCode::PRECONDITION_REQUIRED,
            json!({ "detail": format!("\u{7248}\u{672c} {version} \u{5c1a}\u{672a}\u{51c6}\u{5907}\u{597d}\u{ff0c}\u{8bf7}\u{5148}\u{6267}\u{884c} prepare-update") }),
        )));
    }

    let Some(guard) = SystemUpdateGuard::try_acquire() else {
        return Ok(Err(update_already_running_response()));
    };

    save_previous_release();
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
                tracing::error!(error = %err, "admin system update apply failed");
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

fn save_previous_release() {
    let current = current_symlink_path();
    if let Ok(target) = std::fs::read_link(&current) {
        if let Some(name) = target.file_name().and_then(|n| n.to_str()) {
            let prev_path = aether_base_dir().join(PREVIOUS_RELEASE_FILENAME);
            let _ = std::fs::write(prev_path, name);
        }
    }
}

fn switch_current_symlink(version: &str) -> Result<(), String> {
    let target = release_dir_for_version(version)?;
    if !target.is_dir() {
        return Err(format!(
            "\u{7248}\u{672c}\u{76ee}\u{5f55}\u{4e0d}\u{5b58}\u{5728}: {}",
            target.display()
        ));
    }
    validate_release_payload_dir(&target)?;

    let current = current_symlink_path();
    let current_new = current.with_file_name("current.new");

    let _ = remove_path_if_exists(&current_new);

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &current_new)
        .map_err(|err| format!("\u{521b}\u{5efa}\u{4e34}\u{65f6}\u{7b26}\u{53f7}\u{94fe}\u{63a5}\u{5931}\u{8d25}: {err}"))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &current_new)
        .map_err(|err| format!("\u{521b}\u{5efa}\u{4e34}\u{65f6}\u{7b26}\u{53f7}\u{94fe}\u{63a5}\u{5931}\u{8d25}: {err}"))?;

    std::fs::rename(&current_new, &current)
        .map_err(|err| format!("\u{539f}\u{5b50}\u{5207}\u{6362}\u{7b26}\u{53f7}\u{94fe}\u{63a5}\u{5931}\u{8d25}: {err}"))?;

    Ok(())
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
                let _ = std::fs::remove_file(prev_path);

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
                tracing::error!(error = %err, "admin system rollback failed");
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
    fn update_finds_nested_release_payload_dir() {
        let staging = temp_test_dir("nested");
        let bundle = staging.join("aether-v1.2.3-linux-amd64");
        write_release_payload(&bundle);

        let found = find_release_payload_dir(&staging).expect("payload dir should be found");

        assert_eq!(found, bundle);
        std::fs::remove_dir_all(staging).ok();
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
            "http://github.com/fawney19/Aether/releases/download/v1/aether.tar.gz"
        )
        .is_err());
        assert!(validate_update_download_url("https://example.com/aether.tar.gz").is_err());
    }

    #[test]
    fn update_rejects_archive_path_traversal() {
        assert!(validate_archive_entry_path(Path::new("bundle/bin/aether-gateway")).is_ok());
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
    }
}
