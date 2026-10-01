//! CLI 客户端画像（Codex / Claude Code）的运行时发布与官方版本刷新。
//!
//! 每个客户端由一份 [`CliClientProfileSpec`] 描述：官方 npm 发布源、平台包校验规则、
//! 运行时缓存键、环境变量开关与画像发布函数。刷新逻辑本身与客户端无关。

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use aether_runtime_state::RuntimeState;
use futures_util::StreamExt as _;
use reqwest::{redirect::Policy, Client};
use semver::Version;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::ai_serving::api::{codex_client_version, set_codex_cli_version};
use crate::ai_serving::transport::claude_code::{
    claude_code_client_version, set_claude_code_cli_version,
};
use crate::task_runtime::{TASK_KEY_CLAUDE_CODE_CLIENT_PROFILE, TASK_KEY_CODEX_CLIENT_PROFILE};
use crate::AppState;

const PROFILE_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const PROFILE_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const RELEASE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RELEASE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RELEASE_BYTES: usize = 256 * 1024;

/// 单个 CLI 客户端的发布源、校验规则、缓存与运行时画像发布方式。
pub(crate) struct CliClientProfileSpec {
    /// 日志中的客户端标识。
    client: &'static str,
    /// 官方 npm stable 标签的发布元数据地址。
    release_endpoint: &'static str,
    package_name: &'static str,
    /// 同一发布必须同时携带的平台二进制包。
    platform_targets: &'static [&'static str],
    /// 根据包名、平台与版本给出期望的 optionalDependencies 条目。
    platform_dependency: fn(&str, &str, &str) -> (String, String),
    cache_key: &'static str,
    refresh_env: &'static str,
    fixed_version_env: &'static str,
    task_key: &'static str,
    active_version: fn() -> String,
    publish_version: fn(&str) -> Result<(), &'static str>,
}

fn codex_platform_dependency(package: &str, target: &str, version: &str) -> (String, String) {
    (
        format!("{package}-{target}"),
        format!("npm:{package}@{version}-{target}"),
    )
}

fn claude_code_platform_dependency(package: &str, target: &str, version: &str) -> (String, String) {
    (format!("{package}-{target}"), version.to_owned())
}

fn publish_codex_version(version: &str) -> Result<(), &'static str> {
    set_codex_cli_version(version).map(|_| ())
}

fn publish_claude_code_version(version: &str) -> Result<(), &'static str> {
    set_claude_code_cli_version(version).map(|_| ())
}

pub(crate) static CODEX_CLI_PROFILE: CliClientProfileSpec = CliClientProfileSpec {
    client: "codex",
    release_endpoint: "https://registry.npmjs.org/@openai%2Fcodex/latest",
    package_name: "@openai/codex",
    platform_targets: &[
        "darwin-arm64",
        "darwin-x64",
        "linux-arm64",
        "linux-x64",
        "win32-arm64",
        "win32-x64",
    ],
    platform_dependency: codex_platform_dependency,
    cache_key: "aether:codex:client-profile:v1",
    refresh_env: "AETHER_CODEX_CLIENT_PROFILE_REFRESH",
    fixed_version_env: "AETHER_CODEX_CLIENT_VERSION",
    task_key: TASK_KEY_CODEX_CLIENT_PROFILE,
    active_version: codex_client_version,
    publish_version: publish_codex_version,
};

/// Claude Code 跟随 npm `latest` 标签，与官方 CLI 默认的自动更新通道一致。
/// 仅 CLI 版本（User-Agent 与 billing cc_version）随发布刷新；Stainless SDK 与运行时
/// 指纹仍由 transport crate 中带版本号的身份模板统一维护。
pub(crate) static CLAUDE_CODE_CLI_PROFILE: CliClientProfileSpec = CliClientProfileSpec {
    client: "claude_code",
    release_endpoint: "https://registry.npmjs.org/@anthropic-ai%2Fclaude-code/latest",
    package_name: "@anthropic-ai/claude-code",
    platform_targets: &[
        "darwin-arm64",
        "darwin-x64",
        "linux-arm64",
        "linux-x64",
        "linux-arm64-musl",
        "linux-x64-musl",
        "win32-arm64",
        "win32-x64",
    ],
    platform_dependency: claude_code_platform_dependency,
    cache_key: "aether:claude_code:client-profile:v1",
    refresh_env: "AETHER_CLAUDE_CODE_CLIENT_PROFILE_REFRESH",
    fixed_version_env: "AETHER_CLAUDE_CODE_CLIENT_VERSION",
    task_key: TASK_KEY_CLAUDE_CODE_CLIENT_PROFILE,
    active_version: claude_code_client_version,
    publish_version: publish_claude_code_version,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NpmRelease {
    name: String,
    version: String,
    optional_dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CachedProfile {
    version: String,
    verified_at_unix_secs: u64,
}

#[derive(Debug, thiserror::Error)]
enum ProfileRefreshError {
    #[error("CLI release client initialization failed: {0}")]
    Client(#[from] reqwest::Error),
    #[error("CLI release request returned HTTP {0}")]
    HttpStatus(u16),
    #[error("CLI release response exceeded {MAX_RELEASE_BYTES} bytes")]
    ResponseTooLarge,
    #[error("CLI release metadata is invalid")]
    InvalidMetadata,
    #[error("CLI release version is older than the active profile")]
    Rollback,
    #[error("CLI profile cache operation failed: {0}")]
    Cache(String),
}

fn version_sequence(version: &str) -> Result<u64, ProfileRefreshError> {
    let parsed = Version::parse(version).map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    if !parsed.pre.is_empty()
        || !parsed.build.is_empty()
        || parsed.major > 999
        || parsed.minor > 999
        || parsed.patch > 999
    {
        return Err(ProfileRefreshError::InvalidMetadata);
    }
    Ok(1 + parsed.major * 1_000_000 + parsed.minor * 1_000 + parsed.patch)
}

/// 校验官方 npm 标签及全部平台依赖来自同一版本发布。
fn parse_cli_release(
    spec: &CliClientProfileSpec,
    bytes: &[u8],
) -> Result<String, ProfileRefreshError> {
    if bytes.len() > MAX_RELEASE_BYTES {
        return Err(ProfileRefreshError::ResponseTooLarge);
    }
    let release = serde_json::from_slice::<NpmRelease>(bytes)
        .map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    let sequence = version_sequence(&release.version)?;
    if sequence == 0
        || release.name != spec.package_name
        || spec.platform_targets.iter().any(|target| {
            let (name, expected) =
                (spec.platform_dependency)(spec.package_name, target, &release.version);
            release.optional_dependencies.get(&name) != Some(&expected)
        })
    {
        return Err(ProfileRefreshError::InvalidMetadata);
    }
    Ok(release.version)
}

fn refresh_enabled_from(value: Option<&str>) -> bool {
    !value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off"
        )
    })
}

fn refresh_enabled(spec: &CliClientProfileSpec) -> bool {
    refresh_enabled_from(std::env::var(spec.refresh_env).ok().as_deref())
}

fn fixed_version_from(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || version_sequence(value).is_err() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn fixed_version_override(spec: &CliClientProfileSpec) -> Option<String> {
    let value = std::env::var(spec.fixed_version_env).ok()?;
    let version = fixed_version_from(Some(&value));
    if version.is_none() {
        warn!(
            event_name = "cli_client_profile_fixed_version_invalid",
            client = spec.client,
            env = spec.fixed_version_env,
            "fixed CLI client version is invalid; using cached or built-in profile"
        );
    }
    version
}

fn build_release_client() -> Result<Client, ProfileRefreshError> {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(RELEASE_CONNECT_TIMEOUT)
        .timeout(RELEASE_REQUEST_TIMEOUT)
        .build()
        .map_err(ProfileRefreshError::Client)
}

async fn fetch_latest_cli_version(
    spec: &CliClientProfileSpec,
    client: &Client,
) -> Result<String, ProfileRefreshError> {
    let response = client
        .get(spec.release_endpoint)
        .send()
        .await
        .map_err(ProfileRefreshError::Client)?;
    if !response.status().is_success() {
        return Err(ProfileRefreshError::HttpStatus(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RELEASE_BYTES as u64)
    {
        return Err(ProfileRefreshError::ResponseTooLarge);
    }

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ProfileRefreshError::Client)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RELEASE_BYTES {
            return Err(ProfileRefreshError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_cli_release(spec, &bytes)
}

fn publish(spec: &CliClientProfileSpec, version: &str) -> Result<(), ProfileRefreshError> {
    (spec.publish_version)(version).map_err(|_| ProfileRefreshError::InvalidMetadata)
}

async fn restore_cached_profile(
    spec: &CliClientProfileSpec,
    runtime: &RuntimeState,
) -> Result<(), ProfileRefreshError> {
    let Some(raw) = runtime
        .kv_get(spec.cache_key)
        .await
        .map_err(|err| ProfileRefreshError::Cache(err.to_string()))?
    else {
        return Ok(());
    };
    let cached = serde_json::from_str::<CachedProfile>(&raw)
        .map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    if let Some(version) = cached_version_to_restore(&cached, &(spec.active_version)())? {
        publish(spec, &version)?;
        info!(
            event_name = "cli_client_profile_restored",
            client = spec.client,
            version = %version,
            verified_at_unix_secs = cached.verified_at_unix_secs,
            "restored cached CLI client profile"
        );
    }
    Ok(())
}

fn cached_version_to_restore(
    cached: &CachedProfile,
    active_version: &str,
) -> Result<Option<String>, ProfileRefreshError> {
    let cached_sequence = version_sequence(&cached.version)?;
    let active_sequence = version_sequence(active_version)?;
    Ok((cached_sequence >= active_sequence).then(|| cached.version.clone()))
}

async fn refresh_once_with_fetch<F, Fut>(
    spec: &CliClientProfileSpec,
    runtime: &RuntimeState,
    fixed_version: Option<&str>,
    refresh_is_enabled: bool,
    fetch_latest: F,
) -> Result<String, ProfileRefreshError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, ProfileRefreshError>>,
{
    if let Some(version) = fixed_version {
        publish(spec, version)?;
        return Ok(version.to_owned());
    }

    if let Err(error) = restore_cached_profile(spec, runtime).await {
        // 缓存损坏或暂时不可用不应阻断官方版本检查；当前进程继续使用旧画像。
        warn!(
            event_name = "cli_client_profile_cache_restore_failed",
            client = spec.client,
            error = %error,
            "could not restore cached CLI client profile"
        );
    }
    if !refresh_is_enabled {
        return Ok((spec.active_version)());
    }

    let version = fetch_latest().await?;
    let current = (spec.active_version)();
    if version_sequence(&version)? < version_sequence(&current)? {
        return Err(ProfileRefreshError::Rollback);
    }

    let cached = CachedProfile {
        version: version.clone(),
        verified_at_unix_secs: chrono::Utc::now().timestamp().max(0) as u64,
    };
    let serialized =
        serde_json::to_string(&cached).map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    publish(spec, &version)?;
    if let Err(error) = runtime
        .kv_set(spec.cache_key, serialized, Some(PROFILE_CACHE_TTL))
        .await
    {
        // 本地画像已经完成原子替换；缓存写失败只影响下次进程启动的恢复。
        warn!(
            event_name = "cli_client_profile_cache_write_failed",
            client = spec.client,
            error = %error,
            "published CLI client profile locally but could not persist the cache"
        );
    }
    Ok(version)
}

async fn refresh_once(
    spec: &CliClientProfileSpec,
    runtime: &RuntimeState,
) -> Result<String, ProfileRefreshError> {
    let fixed_version = fixed_version_override(spec);
    refresh_once_with_fetch(
        spec,
        runtime,
        fixed_version.as_deref(),
        refresh_enabled(spec),
        || async {
            let client = build_release_client()?;
            fetch_latest_cli_version(spec, &client).await
        },
    )
    .await
}

pub(crate) async fn prewarm(
    spec: &CliClientProfileSpec,
    runtime: &RuntimeState,
) -> Result<String, String> {
    refresh_once(spec, runtime)
        .await
        .map_err(|err| err.to_string())
}

pub(crate) fn spawn_worker(
    spec: &'static CliClientProfileSpec,
    app: AppState,
) -> tokio::task::JoinHandle<()> {
    crate::task_runtime::spawn_singleton_worker(app, spec.task_key, move |app| async move {
        let mut interval = tokio::time::interval(PROFILE_REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 启动阶段由 prewarm 完成一次检查；后台任务只负责后续每日刷新，避免重复建连。
        interval.tick().await;
        loop {
            interval.tick().await;
            match refresh_once(spec, app.runtime_state()).await {
                Ok(version) => info!(
                    event_name = "cli_client_profile_refreshed",
                    client = spec.client,
                    version = %version,
                    "refreshed CLI client profile"
                ),
                Err(error) => warn!(
                    event_name = "cli_client_profile_refresh_failed",
                    client = spec.client,
                    error = %error,
                    "keeping the previous CLI client profile after refresh failure"
                ),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };
    use std::time::Duration;

    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};

    use super::{
        cached_version_to_restore, fixed_version_from, parse_cli_release, refresh_enabled_from,
        refresh_once_with_fetch, CachedProfile, CliClientProfileSpec, ProfileRefreshError,
        CLAUDE_CODE_CLI_PROFILE, CODEX_CLI_PROFILE,
    };

    const TEST_BUILTIN_VERSION: &str = "1.0.0";

    /// 测试画像使用独立存储，避免改写进程级 Codex / Claude Code 画像而干扰并行测试。
    static TEST_ACTIVE_VERSION: Mutex<String> = Mutex::new(String::new());
    /// 异步测试会跨 await 持有该锁，因此使用 tokio 的异步锁串行化共享测试画像。
    static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn test_active_version() -> String {
        TEST_ACTIVE_VERSION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn test_publish_version(version: &str) -> Result<(), &'static str> {
        *TEST_ACTIVE_VERSION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = version.to_owned();
        Ok(())
    }

    static TEST_PROFILE: CliClientProfileSpec = CliClientProfileSpec {
        client: "test",
        release_endpoint: "https://registry.invalid/test/latest",
        package_name: "@test/cli",
        platform_targets: &["linux-x64"],
        platform_dependency: super::claude_code_platform_dependency,
        cache_key: "aether:test:client-profile:v1",
        refresh_env: "AETHER_TEST_CLIENT_PROFILE_REFRESH",
        fixed_version_env: "AETHER_TEST_CLIENT_VERSION",
        task_key: "maintenance.test.client.profile",
        active_version: test_active_version,
        publish_version: test_publish_version,
    };

    async fn test_profile_guard() -> tokio::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().await;
        test_publish_version(TEST_BUILTIN_VERSION).unwrap();
        guard
    }

    #[test]
    fn accepts_only_one_verified_codex_release_for_all_targets() {
        let body = serde_json::json!({
            "name": "@openai/codex",
            "version": "0.200.1",
            "optionalDependencies": {
                "@openai/codex-darwin-arm64": "npm:@openai/codex@0.200.1-darwin-arm64",
                "@openai/codex-darwin-x64": "npm:@openai/codex@0.200.1-darwin-x64",
                "@openai/codex-linux-arm64": "npm:@openai/codex@0.200.1-linux-arm64",
                "@openai/codex-linux-x64": "npm:@openai/codex@0.200.1-linux-x64",
                "@openai/codex-win32-arm64": "npm:@openai/codex@0.200.1-win32-arm64",
                "@openai/codex-win32-x64": "npm:@openai/codex@0.200.1-win32-x64"
            }
        });
        assert_eq!(
            parse_cli_release(&CODEX_CLI_PROFILE, &serde_json::to_vec(&body).unwrap()).unwrap(),
            "0.200.1"
        );
    }

    #[test]
    fn accepts_only_one_verified_claude_code_release_for_all_targets() {
        let mut body = serde_json::json!({
            "name": "@anthropic-ai/claude-code",
            "version": "2.1.286",
            "optionalDependencies": {
                "@anthropic-ai/claude-code-darwin-arm64": "2.1.286",
                "@anthropic-ai/claude-code-darwin-x64": "2.1.286",
                "@anthropic-ai/claude-code-linux-arm64": "2.1.286",
                "@anthropic-ai/claude-code-linux-x64": "2.1.286",
                "@anthropic-ai/claude-code-linux-arm64-musl": "2.1.286",
                "@anthropic-ai/claude-code-linux-x64-musl": "2.1.286",
                "@anthropic-ai/claude-code-win32-arm64": "2.1.286",
                "@anthropic-ai/claude-code-win32-x64": "2.1.286"
            }
        });
        assert_eq!(
            parse_cli_release(
                &CLAUDE_CODE_CLI_PROFILE,
                &serde_json::to_vec(&body).unwrap()
            )
            .unwrap(),
            "2.1.286"
        );

        // 任一平台包与主包版本不一致都视为未完成的发布。
        body["optionalDependencies"]["@anthropic-ai/claude-code-linux-x64-musl"] =
            serde_json::json!("2.1.285");
        assert!(parse_cli_release(
            &CLAUDE_CODE_CLI_PROFILE,
            &serde_json::to_vec(&body).unwrap()
        )
        .is_err());
    }

    #[test]
    fn rejects_incomplete_or_foreign_release() {
        let incomplete = serde_json::json!({
            "name": "@openai/codex",
            "version": "0.200.1",
            "optionalDependencies": {}
        });
        assert!(parse_cli_release(
            &CODEX_CLI_PROFILE,
            &serde_json::to_vec(&incomplete).unwrap()
        )
        .is_err());

        let foreign = serde_json::json!({
            "name": "@openai/codex",
            "version": "2.1.286",
            "optionalDependencies": {
                "@openai/codex-linux-x64": "2.1.286"
            }
        });
        assert!(parse_cli_release(
            &CLAUDE_CODE_CLI_PROFILE,
            &serde_json::to_vec(&foreign).unwrap()
        )
        .is_err());
    }

    #[test]
    fn client_specs_do_not_share_cache_keys_or_env_switches() {
        assert_ne!(
            CODEX_CLI_PROFILE.cache_key,
            CLAUDE_CODE_CLI_PROFILE.cache_key
        );
        assert_ne!(CODEX_CLI_PROFILE.task_key, CLAUDE_CODE_CLI_PROFILE.task_key);
        assert_ne!(
            CODEX_CLI_PROFILE.refresh_env,
            CLAUDE_CODE_CLI_PROFILE.refresh_env
        );
        assert_ne!(
            CODEX_CLI_PROFILE.fixed_version_env,
            CLAUDE_CODE_CLI_PROFILE.fixed_version_env
        );
        assert_eq!(
            CLAUDE_CODE_CLI_PROFILE.fixed_version_env,
            "AETHER_CLAUDE_CODE_CLIENT_VERSION"
        );
    }

    #[test]
    fn refresh_and_fixed_version_environment_policies_are_strict() {
        assert!(!refresh_enabled_from(Some("off")));
        assert!(!refresh_enabled_from(Some(" FALSE ")));
        assert!(refresh_enabled_from(None));
        assert_eq!(
            fixed_version_from(Some(" 0.200.1 ")).as_deref(),
            Some("0.200.1")
        );
        assert!(fixed_version_from(Some("0.200.1-beta.1")).is_none());
        assert!(fixed_version_from(Some("1.2")).is_none());
    }

    #[test]
    fn cached_profile_never_rewinds_active_profile() {
        let cached = CachedProfile {
            version: "0.200.1".to_string(),
            verified_at_unix_secs: 1,
        };
        assert_eq!(
            cached_version_to_restore(&cached, "0.200.0").unwrap(),
            Some("0.200.1".to_string())
        );
        assert_eq!(cached_version_to_restore(&cached, "0.201.0").unwrap(), None);
    }

    #[tokio::test]
    async fn cache_hit_is_restored_without_network_when_refresh_is_disabled() {
        let _guard = test_profile_guard().await;
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        runtime
            .kv_set(
                TEST_PROFILE.cache_key,
                serde_json::to_string(&CachedProfile {
                    version: "1.2.0".to_string(),
                    verified_at_unix_secs: 1,
                })
                .unwrap(),
                Some(Duration::from_secs(60)),
            )
            .await
            .unwrap();

        let result = refresh_once_with_fetch(&TEST_PROFILE, &runtime, None, false, || async {
            Err(ProfileRefreshError::HttpStatus(599))
        })
        .await
        .unwrap();

        assert_eq!(result, "1.2.0");
        assert_eq!(test_active_version(), "1.2.0");
    }

    #[tokio::test]
    async fn successful_refresh_publishes_and_caches_profile() {
        let _guard = test_profile_guard().await;
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let result = refresh_once_with_fetch(&TEST_PROFILE, &runtime, None, true, || async {
            Ok("1.3.0".to_string())
        })
        .await
        .unwrap();

        assert_eq!(result, "1.3.0");
        assert_eq!(test_active_version(), "1.3.0");
        let cached = runtime
            .kv_get(TEST_PROFILE.cache_key)
            .await
            .unwrap()
            .expect("cached profile");
        let cached = serde_json::from_str::<CachedProfile>(&cached).unwrap();
        assert_eq!(cached.version, "1.3.0");
    }

    #[tokio::test]
    async fn refresh_failure_keeps_previous_profile() {
        let _guard = test_profile_guard().await;
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let result = refresh_once_with_fetch(&TEST_PROFILE, &runtime, None, true, || async {
            Err(ProfileRefreshError::HttpStatus(503))
        })
        .await;

        assert!(matches!(result, Err(ProfileRefreshError::HttpStatus(503))));
        assert_eq!(test_active_version(), TEST_BUILTIN_VERSION);
    }

    #[tokio::test]
    async fn fixed_version_override_skips_network_and_publishes_profile() {
        let _guard = test_profile_guard().await;
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let fetch_called = AtomicBool::new(false);
        let result =
            refresh_once_with_fetch(&TEST_PROFILE, &runtime, Some("1.5.0"), true, || async {
                fetch_called.store(true, Ordering::SeqCst);
                Ok("1.6.0".to_string())
            })
            .await
            .unwrap();

        assert_eq!(result, "1.5.0");
        assert!(!fetch_called.load(Ordering::SeqCst));
        assert_eq!(test_active_version(), "1.5.0");
    }

    #[tokio::test]
    async fn rollback_is_rejected_without_replacing_profile() {
        let _guard = test_profile_guard().await;
        (TEST_PROFILE.publish_version)("1.5.0").unwrap();
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let result = refresh_once_with_fetch(&TEST_PROFILE, &runtime, None, true, || async {
            Ok("1.4.9".to_string())
        })
        .await;

        assert!(matches!(result, Err(ProfileRefreshError::Rollback)));
        assert_eq!(test_active_version(), "1.5.0");
    }
}
