//! Grok CLI 客户端版本的运行时发布与官方版本刷新。
//!
//! cli-chat-proxy.grok.com 会对低于最低版本的 `x-grok-client-version` 直接返回 426，
//! 因此网关定期读取官方发布渠道并原子替换传输层使用的版本号。

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use aether_runtime_state::RuntimeState;
use futures_util::StreamExt as _;
use reqwest::{redirect::Policy, Client};
use semver::Version;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::provider_transport::{set_xai_client_version, xai_client_version};
use crate::AppState;

/// 官方安装脚本读取的 stable 渠道，响应体是纯文本版本号。
const CLI_STABLE_CHANNEL_ENDPOINT: &str = "https://x.ai/cli/stable";
/// stable 渠道不可达时（部分部署地区无法直连 x.ai）退回 npm 发布元数据。
const CLI_NPM_RELEASE_ENDPOINT: &str = "https://registry.npmjs.org/@xai-official%2Fgrok/latest";
const CLI_NPM_PACKAGE: &str = "@xai-official/grok";
const PROFILE_CACHE_KEY: &str = "aether:xai:client-profile:v1";
const PROFILE_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// xAI 会在发布后很快抬高最低版本，刷新间隔比 Codex 更短。
const PROFILE_REFRESH_INTERVAL: Duration = Duration::from_secs(3 * 60 * 60);
const RELEASE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RELEASE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RELEASE_BYTES: usize = 256 * 1024;
const CLI_TARGETS: [&str; 6] = [
    "darwin-arm64",
    "darwin-x64",
    "linux-arm64",
    "linux-x64",
    "win32-arm64",
    "win32-x64",
];

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
    #[error("Grok CLI release client initialization failed: {0}")]
    Client(#[from] reqwest::Error),
    #[error("Grok CLI release request returned HTTP {0}")]
    HttpStatus(u16),
    #[error("Grok CLI release response exceeded {MAX_RELEASE_BYTES} bytes")]
    ResponseTooLarge,
    #[error("Grok CLI release metadata is invalid")]
    InvalidMetadata,
    #[error("Grok CLI release version is older than the active profile")]
    Rollback,
    #[error("Grok CLI profile cache operation failed: {0}")]
    Cache(String),
    #[error("Grok CLI stable channel failed ({stable}); npm fallback failed ({npm})")]
    AllSourcesFailed { stable: String, npm: String },
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

/// stable 渠道只返回一行版本号；任何多余内容都视为异常响应（例如被劫持的 HTML 页面）。
fn parse_stable_channel(bytes: &[u8]) -> Result<String, ProfileRefreshError> {
    if bytes.len() > MAX_RELEASE_BYTES {
        return Err(ProfileRefreshError::ResponseTooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    let version = text.trim();
    version_sequence(version)?;
    Ok(version.to_owned())
}

/// 校验 npm latest 标签及六个平台二进制包来自同一版本发布。
fn parse_npm_release(bytes: &[u8]) -> Result<String, ProfileRefreshError> {
    if bytes.len() > MAX_RELEASE_BYTES {
        return Err(ProfileRefreshError::ResponseTooLarge);
    }
    let release = serde_json::from_slice::<NpmRelease>(bytes)
        .map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    version_sequence(&release.version)?;
    if release.name != CLI_NPM_PACKAGE
        || CLI_TARGETS.iter().any(|target| {
            release
                .optional_dependencies
                .get(&format!("{CLI_NPM_PACKAGE}-{target}"))
                != Some(&release.version)
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

fn refresh_enabled() -> bool {
    refresh_enabled_from(
        std::env::var("AETHER_XAI_CLIENT_PROFILE_REFRESH")
            .ok()
            .as_deref(),
    )
}

fn fixed_version_from(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || version_sequence(value).is_err() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn fixed_version_override() -> Option<String> {
    let value = std::env::var("AETHER_XAI_CLIENT_VERSION").ok()?;
    let version = fixed_version_from(Some(&value));
    if version.is_none() {
        warn!(
            event_name = "xai_client_profile_fixed_version_invalid",
            "AETHER_XAI_CLIENT_VERSION is invalid; using cached or built-in profile"
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

async fn fetch_bounded(client: &Client, url: &str) -> Result<Vec<u8>, ProfileRefreshError> {
    let response = client
        .get(url)
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
    Ok(bytes)
}

async fn fetch_latest_with_fallback<S, SFut, N, NFut>(
    fetch_stable: S,
    fetch_npm: N,
) -> Result<String, ProfileRefreshError>
where
    S: FnOnce() -> SFut,
    SFut: Future<Output = Result<String, ProfileRefreshError>>,
    N: FnOnce() -> NFut,
    NFut: Future<Output = Result<String, ProfileRefreshError>>,
{
    let stable_error = match fetch_stable().await {
        Ok(version) => return Ok(version),
        Err(error) => error,
    };
    fetch_npm()
        .await
        .map_err(|npm_error| ProfileRefreshError::AllSourcesFailed {
            stable: stable_error.to_string(),
            npm: npm_error.to_string(),
        })
}

async fn fetch_latest_cli_version(client: &Client) -> Result<String, ProfileRefreshError> {
    fetch_latest_with_fallback(
        || async {
            let bytes = fetch_bounded(client, CLI_STABLE_CHANNEL_ENDPOINT).await?;
            parse_stable_channel(&bytes)
        },
        || async {
            let bytes = fetch_bounded(client, CLI_NPM_RELEASE_ENDPOINT).await?;
            parse_npm_release(&bytes)
        },
    )
    .await
}

fn publish_version(version: &str) -> Result<(), ProfileRefreshError> {
    set_xai_client_version(version)
        .map(|_| ())
        .map_err(|_| ProfileRefreshError::InvalidMetadata)
}

async fn restore_cached_profile(runtime: &RuntimeState) -> Result<(), ProfileRefreshError> {
    let Some(raw) = runtime
        .kv_get(PROFILE_CACHE_KEY)
        .await
        .map_err(|err| ProfileRefreshError::Cache(err.to_string()))?
    else {
        return Ok(());
    };
    let cached = serde_json::from_str::<CachedProfile>(&raw)
        .map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    if let Some(version) = cached_version_to_restore(&cached, &xai_client_version())? {
        publish_version(&version)?;
        info!(
            event_name = "xai_client_profile_restored",
            version = %version,
            verified_at_unix_secs = cached.verified_at_unix_secs,
            "restored cached Grok CLI profile"
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
    Ok((cached_sequence > active_sequence).then(|| cached.version.clone()))
}

async fn refresh_once_with_fetch<F, Fut>(
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
        publish_version(version)?;
        return Ok(version.to_owned());
    }

    if let Err(error) = restore_cached_profile(runtime).await {
        // 缓存损坏或暂时不可用不应阻断官方版本检查；当前进程继续使用旧画像。
        warn!(
            event_name = "xai_client_profile_cache_restore_failed",
            error = %error,
            "could not restore cached Grok CLI profile"
        );
    }
    if !refresh_is_enabled {
        return Ok(xai_client_version());
    }

    let version = fetch_latest().await?;
    let current = xai_client_version();
    if version_sequence(&version)? < version_sequence(&current)? {
        return Err(ProfileRefreshError::Rollback);
    }

    let cached = CachedProfile {
        version: version.clone(),
        verified_at_unix_secs: chrono::Utc::now().timestamp().max(0) as u64,
    };
    let serialized =
        serde_json::to_string(&cached).map_err(|_| ProfileRefreshError::InvalidMetadata)?;
    publish_version(&version)?;
    if let Err(error) = runtime
        .kv_set(PROFILE_CACHE_KEY, serialized, Some(PROFILE_CACHE_TTL))
        .await
    {
        // 本地版本已经完成原子替换；缓存写失败只影响下次进程启动的恢复。
        warn!(
            event_name = "xai_client_profile_cache_write_failed",
            error = %error,
            "published Grok CLI profile locally but could not persist the cache"
        );
    }
    Ok(version)
}

async fn refresh_once(runtime: &RuntimeState) -> Result<String, ProfileRefreshError> {
    let fixed_version = fixed_version_override();
    refresh_once_with_fetch(
        runtime,
        fixed_version.as_deref(),
        refresh_enabled(),
        || async {
            let client = build_release_client()?;
            fetch_latest_cli_version(&client).await
        },
    )
    .await
}

pub(crate) async fn prewarm(runtime: &RuntimeState) -> Result<String, String> {
    refresh_once(runtime).await.map_err(|err| err.to_string())
}

pub(crate) fn spawn_worker(app: AppState) -> tokio::task::JoinHandle<()> {
    crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_XAI_CLIENT_PROFILE,
        |app| async move {
            let mut interval = tokio::time::interval(PROFILE_REFRESH_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // 启动阶段由 prewarm 完成一次检查；后台任务只负责后续定时刷新，避免重复建连。
            interval.tick().await;
            loop {
                interval.tick().await;
                match refresh_once(app.runtime_state()).await {
                    Ok(version) => info!(
                        event_name = "xai_client_profile_refreshed",
                        version = %version,
                        "refreshed Grok CLI profile"
                    ),
                    Err(error) => warn!(
                        event_name = "xai_client_profile_refresh_failed",
                        error = %error,
                        "keeping the previous Grok CLI profile after refresh failure"
                    ),
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    };
    use std::time::Duration;

    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};

    use super::{
        cached_version_to_restore, fetch_latest_with_fallback, fixed_version_from,
        parse_npm_release, parse_stable_channel, refresh_enabled_from, refresh_once_with_fetch,
        CachedProfile, ProfileRefreshError, PROFILE_CACHE_KEY,
    };
    use crate::provider_transport::{set_xai_client_version, xai_client_version};

    static PROFILE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct VersionRestore(String);

    impl Drop for VersionRestore {
        fn drop(&mut self) {
            let _ = set_xai_client_version(&self.0);
        }
    }

    fn version_restore_guard() -> (std::sync::MutexGuard<'static, ()>, VersionRestore) {
        let lock = PROFILE_TEST_LOCK.get_or_init(|| Mutex::new(()));
        let guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let restore = VersionRestore(xai_client_version());
        (guard, restore)
    }

    fn npm_release(version: &str) -> serde_json::Value {
        let mut deps = serde_json::Map::new();
        for target in super::CLI_TARGETS {
            deps.insert(
                format!("@xai-official/grok-{target}"),
                serde_json::Value::String(version.to_string()),
            );
        }
        serde_json::json!({
            "name": "@xai-official/grok",
            "version": version,
            "optionalDependencies": deps,
        })
    }

    #[test]
    fn stable_channel_accepts_only_a_bare_release_version() {
        assert_eq!(parse_stable_channel(b"1.0.46\n").unwrap(), "1.0.46");
        assert!(parse_stable_channel(b"<html>1.0.46</html>").is_err());
        assert!(parse_stable_channel(b"1.0.47-alpha.1").is_err());
        assert!(parse_stable_channel(b"").is_err());
    }

    #[test]
    fn npm_release_requires_every_platform_binary_at_the_same_version() {
        let body = npm_release("1.0.46");
        assert_eq!(
            parse_npm_release(&serde_json::to_vec(&body).unwrap()).unwrap(),
            "1.0.46"
        );

        let mut mismatched = npm_release("1.0.46");
        mismatched["optionalDependencies"]["@xai-official/grok-linux-x64"] =
            serde_json::Value::String("1.0.45".to_string());
        assert!(parse_npm_release(&serde_json::to_vec(&mismatched).unwrap()).is_err());

        let mut wrong_package = npm_release("1.0.46");
        wrong_package["name"] = serde_json::Value::String("grok".to_string());
        assert!(parse_npm_release(&serde_json::to_vec(&wrong_package).unwrap()).is_err());
    }

    #[test]
    fn refresh_and_fixed_version_environment_policies_are_strict() {
        assert!(!refresh_enabled_from(Some("off")));
        assert!(!refresh_enabled_from(Some(" FALSE ")));
        assert!(refresh_enabled_from(None));
        assert_eq!(
            fixed_version_from(Some(" 1.0.46 ")).as_deref(),
            Some("1.0.46")
        );
        assert!(fixed_version_from(Some("1.0.46-beta.1")).is_none());
        assert!(fixed_version_from(Some("1.0")).is_none());
    }

    #[test]
    fn cached_profile_never_rewinds_active_profile() {
        let cached = CachedProfile {
            version: "1.0.50".to_string(),
            verified_at_unix_secs: 1,
        };
        assert_eq!(
            cached_version_to_restore(&cached, "1.0.46").unwrap(),
            Some("1.0.50".to_string())
        );
        assert_eq!(cached_version_to_restore(&cached, "1.1.0").unwrap(), None);
    }

    #[tokio::test]
    async fn npm_is_used_only_when_the_stable_channel_fails() {
        let npm_called = AtomicBool::new(false);
        let version = fetch_latest_with_fallback(
            || async { Ok("1.0.46".to_string()) },
            || async {
                npm_called.store(true, Ordering::SeqCst);
                Ok("1.0.45".to_string())
            },
        )
        .await
        .unwrap();
        assert_eq!(version, "1.0.46");
        assert!(!npm_called.load(Ordering::SeqCst));

        let version = fetch_latest_with_fallback(
            || async { Err(ProfileRefreshError::HttpStatus(503)) },
            || async { Ok("1.0.46".to_string()) },
        )
        .await
        .unwrap();
        assert_eq!(version, "1.0.46");

        let result = fetch_latest_with_fallback(
            || async { Err(ProfileRefreshError::HttpStatus(503)) },
            || async { Err(ProfileRefreshError::InvalidMetadata) },
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileRefreshError::AllSourcesFailed { .. })
        ));
    }

    #[tokio::test]
    async fn cache_hit_is_restored_without_network_when_refresh_is_disabled() {
        let (_lock, _restore) = version_restore_guard();
        set_xai_client_version("1.0.46").unwrap();
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        runtime
            .kv_set(
                PROFILE_CACHE_KEY,
                serde_json::to_string(&CachedProfile {
                    version: "1.0.50".to_string(),
                    verified_at_unix_secs: 1,
                })
                .unwrap(),
                Some(Duration::from_secs(60)),
            )
            .await
            .unwrap();

        let result = refresh_once_with_fetch(&runtime, None, false, || async {
            Err(ProfileRefreshError::HttpStatus(599))
        })
        .await
        .unwrap();

        assert_eq!(result, "1.0.50");
        assert_eq!(xai_client_version(), "1.0.50");
    }

    #[tokio::test]
    async fn refresh_failure_keeps_previous_profile() {
        let (_lock, _restore) = version_restore_guard();
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let before = xai_client_version();
        let result = refresh_once_with_fetch(&runtime, None, true, || async {
            Err(ProfileRefreshError::HttpStatus(503))
        })
        .await;

        assert!(matches!(result, Err(ProfileRefreshError::HttpStatus(503))));
        assert_eq!(xai_client_version(), before);
    }

    #[tokio::test]
    async fn successful_refresh_publishes_and_caches_version() {
        let (_lock, _restore) = version_restore_guard();
        set_xai_client_version("1.0.46").unwrap();
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let result =
            refresh_once_with_fetch(&runtime, None, true, || async { Ok("1.0.51".to_string()) })
                .await
                .unwrap();

        assert_eq!(result, "1.0.51");
        assert_eq!(xai_client_version(), "1.0.51");
        let cached = runtime.kv_get(PROFILE_CACHE_KEY).await.unwrap().unwrap();
        assert!(cached.contains("\"1.0.51\""));
    }

    #[tokio::test]
    async fn fixed_version_override_skips_network_and_publishes_version() {
        let (_lock, _restore) = version_restore_guard();
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let fetch_called = AtomicBool::new(false);
        let result = refresh_once_with_fetch(&runtime, Some("1.0.60"), true, || async {
            fetch_called.store(true, Ordering::SeqCst);
            Ok("1.0.61".to_string())
        })
        .await
        .unwrap();

        assert_eq!(result, "1.0.60");
        assert!(!fetch_called.load(Ordering::SeqCst));
        assert_eq!(xai_client_version(), "1.0.60");
    }

    #[tokio::test]
    async fn rollback_is_rejected_without_replacing_profile() {
        let (_lock, _restore) = version_restore_guard();
        set_xai_client_version("1.0.60").unwrap();
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let result =
            refresh_once_with_fetch(&runtime, None, true, || async { Ok("1.0.59".to_string()) })
                .await;

        assert!(matches!(result, Err(ProfileRefreshError::Rollback)));
        assert_eq!(xai_client_version(), "1.0.60");
    }
}
