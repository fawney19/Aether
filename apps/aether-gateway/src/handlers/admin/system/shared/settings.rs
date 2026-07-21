use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::build_admin_usage_counter_health_payload;
use crate::handlers::admin::system::shared::update::{
    current_self_update_blocker, self_update_supported,
};
use crate::handlers::admin::system::shared::update_client::{
    build_direct_update_http_client, build_update_http_client, has_explicit_update_proxy_env,
    update_github_token_from_env,
};
use crate::handlers::shared::{system_config_bool, system_config_string};
use crate::GatewayError;
use aether_admin::system::{
    build_admin_api_formats_payload as build_admin_api_formats_payload_pure,
    build_admin_system_check_update_payload as build_admin_system_check_update_payload_pure,
    build_admin_system_check_update_payload_with_release, build_admin_system_releases_payload,
    build_admin_system_settings_payload as build_admin_system_settings_payload_pure,
    build_admin_system_settings_updated_payload,
    build_admin_system_stats_payload as build_admin_system_stats_payload_pure,
    parse_admin_system_settings_update, AdminSystemUpdateRelease,
};
use axum::body::Bytes;
use axum::http;
#[cfg(not(test))]
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const AETHER_RELEASES_API_URL: &str =
    "https://api.github.com/repos/fawney19/Aether/releases?per_page=20";
const AETHER_RELEASE_TAG_URL_BASE: &str = "https://github.com/fawney19/Aether/releases/tag";
const SOURCE_BUILD_UPDATE_BLOCKER: &str = "当前为源码构建，请使用 git pull 后重新编译。";
const SOURCE_BUILD_RELEASE_BLOCKER: &str = "当前为源码构建，请手动切换到对应标签后重新编译。";

/// Minimum interval between actual GitHub API requests.  Within this window
/// the cached result is reused.
const RELEASE_CACHE_TTL: Duration = Duration::from_secs(1200);

pub(crate) fn current_aether_version() -> String {
    option_env!("AETHER_BUILD_VERSION")
        .filter(|version| !version.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string()
}

pub(crate) fn build_admin_system_check_update_payload() -> serde_json::Value {
    build_admin_system_check_update_payload_pure(current_aether_version())
}

pub(crate) fn build_admin_system_check_update_payload_from_release(
    latest_release: Option<AdminSystemUpdateRelease>,
    error: Option<String>,
) -> serde_json::Value {
    let mut payload = build_admin_system_check_update_payload_with_release(
        current_aether_version(),
        latest_release,
        error,
    );
    apply_self_update_check_update_override(&mut payload, self_update_supported());
    payload
}

pub(crate) fn build_admin_system_releases_list_payload(
    releases: Vec<AdminSystemUpdateRelease>,
    error: Option<String>,
) -> serde_json::Value {
    let mut payload =
        build_admin_system_releases_payload(current_aether_version(), releases, error);
    apply_self_update_releases_override(&mut payload, self_update_supported());
    payload
}

fn current_build_is_release() -> bool {
    option_env!("AETHER_BUILD_TYPE").unwrap_or("source") == "release"
}

fn apply_self_update_check_update_override(payload: &mut Value, supported: bool) {
    apply_self_update_check_update_override_with_blocker(
        payload,
        supported,
        current_self_update_blocker(),
    );
}

fn apply_self_update_check_update_override_with_blocker(
    payload: &mut Value,
    supported: bool,
    blocker: &str,
) {
    if supported {
        return;
    }
    if payload.get("has_update").and_then(Value::as_bool) != Some(true) {
        return;
    }

    payload["updatable"] = json!(false);
    payload["update_blocker"] = json!(blocker);
}

fn apply_self_update_releases_override(payload: &mut Value, supported: bool) {
    apply_self_update_releases_override_with_blocker(
        payload,
        supported,
        current_self_update_release_blocker(),
    );
}

fn apply_self_update_releases_override_with_blocker(
    payload: &mut Value,
    supported: bool,
    blocker: &str,
) {
    if supported {
        return;
    }
    let Some(releases) = payload.get_mut("releases").and_then(Value::as_array_mut) else {
        return;
    };

    for release in releases {
        if release.get("is_current").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        release["updatable"] = json!(false);
        if release.get("update_blocker").is_none() || release["update_blocker"].is_null() {
            release["update_blocker"] = json!(blocker);
        }
    }
}

fn current_self_update_release_blocker() -> &'static str {
    if !current_build_is_release() {
        return SOURCE_BUILD_RELEASE_BLOCKER;
    }

    if self_update_supported() {
        ""
    } else {
        current_self_update_blocker()
    }
}

#[cfg(not(test))]
struct CachedReleases {
    all: Vec<AdminSystemUpdateRelease>,
    latest: Option<AdminSystemUpdateRelease>,
    error: Option<String>,
    fetched_at: std::time::Instant,
}

#[cfg(not(test))]
fn releases_cache() -> &'static std::sync::Mutex<Option<CachedReleases>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<CachedReleases>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(not(test))]
async fn ensure_releases_cached(force: bool) {
    {
        if let Ok(guard) = releases_cache().lock() {
            if let Some(cached) = guard.as_ref() {
                if should_reuse_releases_cache(
                    force,
                    cached.error.is_some(),
                    cached.fetched_at.elapsed(),
                ) {
                    return;
                }
            }
        }
    }

    let (all, latest, error) = match fetch_admin_system_releases_inner().await {
        Ok(releases) => {
            let latest = releases.first().cloned();
            (releases, latest, None)
        }
        Err(err) => (Vec::new(), None, Some(err)),
    };

    if let Ok(mut guard) = releases_cache().lock() {
        *guard = Some(CachedReleases {
            all,
            latest,
            error,
            fetched_at: std::time::Instant::now(),
        });
    }
}

fn should_reuse_releases_cache(force: bool, has_error: bool, age: Duration) -> bool {
    !force && !has_error && age < RELEASE_CACHE_TTL
}

#[cfg(not(test))]
pub(crate) async fn fetch_latest_admin_system_release(
    force: bool,
) -> (Option<AdminSystemUpdateRelease>, Option<String>) {
    ensure_releases_cached(force).await;
    if let Ok(guard) = releases_cache().lock() {
        if let Some(cached) = guard.as_ref() {
            return (cached.latest.clone(), cached.error.clone());
        }
    }
    (None, None)
}

#[cfg(not(test))]
pub(crate) async fn fetch_admin_system_releases(
    force: bool,
) -> (Vec<AdminSystemUpdateRelease>, Option<String>) {
    ensure_releases_cached(force).await;
    if let Ok(guard) = releases_cache().lock() {
        if let Some(cached) = guard.as_ref() {
            return (cached.all.clone(), cached.error.clone());
        }
    }
    (Vec::new(), None)
}

#[cfg(test)]
pub(crate) async fn fetch_admin_system_releases(
    _force: bool,
) -> (Vec<AdminSystemUpdateRelease>, Option<String>) {
    (
        Vec::new(),
        Some("测试环境未请求 GitHub Releases".to_string()),
    )
}

#[cfg(test)]
pub(crate) async fn fetch_latest_admin_system_release(
    _force: bool,
) -> (Option<AdminSystemUpdateRelease>, Option<String>) {
    (None, Some("测试环境未请求 GitHub Releases".to_string()))
}

#[cfg(not(test))]
pub(crate) async fn resolve_update_target(
    version: Option<String>,
) -> Result<(String, String, Option<String>), (http::StatusCode, serde_json::Value)> {
    let (releases, error) = fetch_admin_system_releases(false).await;
    if releases.is_empty() {
        return Err((
            http::StatusCode::SERVICE_UNAVAILABLE,
            json!({ "detail": error.unwrap_or_else(|| "无法获取版本信息".to_string()) }),
        ));
    }

    let release = match version {
        Some(ref v) => releases.into_iter().find(|r| r.version == *v),
        None => releases.into_iter().next(),
    };

    let release = release.ok_or_else(|| {
        (
            http::StatusCode::NOT_FOUND,
            json!({ "detail": "未找到指定版本" }),
        )
    })?;

    let tarball_url = release.tarball_url.ok_or_else(|| {
        (
            http::StatusCode::PRECONDITION_REQUIRED,
            json!({ "detail": format!("版本 {} 没有适用于当前平台的安装包", release.version) }),
        )
    })?;

    let sha256sums_url = release.sha256sums_url.ok_or_else(|| {
        (
            http::StatusCode::PRECONDITION_REQUIRED,
            json!({ "detail": format!("版本 {} 缺少 SHA256SUMS 校验文件，已拒绝在线更新", release.version) }),
        )
    })?;

    Ok((release.version, tarball_url, Some(sha256sums_url)))
}

#[cfg(test)]
pub(crate) async fn resolve_update_target(
    _version: Option<String>,
) -> Result<(String, String, Option<String>), (http::StatusCode, serde_json::Value)> {
    Err((
        http::StatusCode::SERVICE_UNAVAILABLE,
        json!({ "detail": "测试环境不支持更新" }),
    ))
}

#[cfg(not(test))]
async fn fetch_admin_system_releases_inner() -> Result<Vec<AdminSystemUpdateRelease>, String> {
    let current_channel = update_channel_for_version(&current_aether_version());
    let timeout = Duration::from_secs(8);
    let github_token = update_github_token_from_env();
    let client = build_update_http_client(timeout, "更新检查")?;
    let releases = match fetch_github_releases_with_client(&client, github_token.as_deref()).await {
        Ok(releases) => releases,
        Err(err) if err.rate_limited && !has_explicit_update_proxy_env() => {
            let direct_client = build_direct_update_http_client(timeout, "更新检查直连重试")?;
            fetch_github_releases_with_client(&direct_client, github_token.as_deref())
                .await
                .map_err(|retry_err| retry_err.message)?
        }
        Err(err) => return Err(err.message),
    };

    Ok(releases
        .into_iter()
        .filter(|release| should_include_release_for_channel(release, current_channel))
        .map(|release| {
            let (tarball_url, sha256sums_url) = select_release_tarball_urls(&release);
            AdminSystemUpdateRelease {
                version: release.tag_name.clone(),
                release_url: Some(github_release_tag_url(&release.tag_name)),
                release_notes: release.body.filter(|body| !body.trim().is_empty()),
                published_at: release.published_at,
                tarball_url,
                sha256sums_url,
            }
        })
        .collect())
}

#[derive(Debug)]
struct GitHubReleaseFetchError {
    message: String,
    rate_limited: bool,
}

#[cfg(not(test))]
async fn fetch_github_releases_with_client(
    client: &reqwest::Client,
    github_token: Option<&str>,
) -> Result<Vec<GitHubRelease>, GitHubReleaseFetchError> {
    let mut request = client
        .get(AETHER_RELEASES_API_URL)
        .header(reqwest::header::USER_AGENT, "Aether-Gateway update-check")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    if let Some(token) = github_token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .map_err(|err| GitHubReleaseFetchError {
            message: format!("请求 GitHub Releases 失败: {err}"),
            rate_limited: false,
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(github_release_response_error(status, &body));
    }

    response
        .json()
        .await
        .map_err(|err| GitHubReleaseFetchError {
            message: format!("解析 GitHub Releases 失败: {err}"),
            rate_limited: false,
        })
}

fn github_release_response_error(
    status: reqwest::StatusCode,
    response_body: &str,
) -> GitHubReleaseFetchError {
    if is_github_rate_limit_error(status, response_body) {
        return GitHubReleaseFetchError {
            message: "GitHub Releases API 已触发限流；当前共享代理出口的匿名额度已用尽，更新检查将自动尝试直连。若仍失败，请配置 AETHER_UPDATE_GITHUB_TOKEN / GITHUB_TOKEN / GH_TOKEN，或为 GitHub 更新检查单独设置可用代理。".to_string(),
            rate_limited: true,
        };
    }

    let detail = parse_github_error_message(response_body);
    let message = if detail.is_empty() {
        format!("GitHub Releases 返回错误: HTTP {status}")
    } else {
        format!("GitHub Releases 返回错误: HTTP {status}; {detail}")
    };
    GitHubReleaseFetchError {
        message,
        rate_limited: false,
    }
}

fn is_github_rate_limit_error(status: reqwest::StatusCode, response_body: &str) -> bool {
    status == reqwest::StatusCode::FORBIDDEN
        && response_body
            .to_ascii_lowercase()
            .contains("rate limit exceeded")
}

fn parse_github_error_message(response_body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(response_body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(|message| message.as_str())
                .map(str::trim)
                .filter(|message| !message.is_empty())
                .map(ToString::to_string)
        })
        .unwrap_or_default()
}

#[cfg(not(test))]
fn should_include_release_for_channel(
    release: &GitHubRelease,
    current_channel: UpdateChannel,
) -> bool {
    if release.draft || !release.tag_name.starts_with('v') {
        return false;
    }
    if !release.prerelease {
        return true;
    }
    current_channel.allows_prerelease(&release.tag_name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateChannel {
    Stable,
    Rc,
    Beta,
    OtherPrerelease,
}

impl UpdateChannel {
    fn allows_prerelease(self, release_version: &str) -> bool {
        match self {
            Self::Stable => false,
            Self::Rc => update_channel_for_version(release_version) == Self::Rc,
            Self::Beta => update_channel_for_version(release_version) == Self::Beta,
            Self::OtherPrerelease => {
                matches!(
                    update_channel_for_version(release_version),
                    Self::Rc | Self::Beta | Self::OtherPrerelease
                )
            }
        }
    }
}

fn update_channel_for_version(version: &str) -> UpdateChannel {
    let normalized = version
        .trim()
        .strip_prefix('v')
        .or_else(|| version.trim().strip_prefix('V'))
        .unwrap_or(version.trim());
    let Some((_, prerelease)) = normalized.split_once('-') else {
        return UpdateChannel::Stable;
    };
    let prerelease = prerelease.to_ascii_lowercase();
    if prerelease.starts_with("rc") {
        UpdateChannel::Rc
    } else if prerelease.starts_with("beta") {
        UpdateChannel::Beta
    } else {
        UpdateChannel::OtherPrerelease
    }
}

#[cfg(not(test))]
#[derive(Debug, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(not(test))]
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubReleaseAsset>,
}

fn github_release_tag_url(tag_name: &str) -> String {
    format!("{AETHER_RELEASE_TAG_URL_BASE}/{tag_name}")
}

#[cfg(not(test))]
fn select_release_tarball_urls(release: &GitHubRelease) -> (Option<String>, Option<String>) {
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
    let expected_name = format!("aether-{}-{}-{}.tar.gz", release.tag_name, platform, arch);

    let tarball_url = release
        .assets
        .iter()
        .find(|a| a.name == expected_name)
        .map(|a| a.browser_download_url.clone());

    let sha256sums_url = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .map(|a| a.browser_download_url.clone());

    (tarball_url, sha256sums_url)
}

pub(crate) async fn build_admin_system_stats_payload(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let providers = state
        .list_provider_catalog_providers(false)
        .await
        .unwrap_or_default();
    let total_providers = providers.len() as u64;
    let active_providers = providers
        .iter()
        .filter(|provider| provider.is_active)
        .count() as u64;
    let stats = state.read_admin_system_stats().await?;
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let usage_counter_snapshot = state
        .as_ref()
        .read_cached_usage_counter_health()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let usage_counter =
        build_admin_usage_counter_health_payload(&usage_counter_snapshot, now_unix_secs);

    Ok(build_admin_system_stats_payload_pure(
        stats.total_users,
        stats.active_users,
        total_providers,
        active_providers,
        stats.total_api_keys,
        stats.total_requests,
        usage_counter,
    ))
}

pub(crate) async fn build_admin_system_settings_payload(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let default_provider_config = state
        .read_system_config_json_value("default_provider")
        .await?;
    let default_model_config = state.read_system_config_json_value("default_model").await?;
    let enable_usage_tracking_config = state
        .read_system_config_json_value("enable_usage_tracking")
        .await?;
    let password_policy_level_config = state
        .read_system_config_json_value("password_policy_level")
        .await?;

    let default_provider = match system_config_string(default_provider_config.as_ref()) {
        Some(value) => Some(value),
        None => state
            .list_provider_catalog_providers(false)
            .await
            .ok()
            .unwrap_or_default()
            .into_iter()
            .find(|provider| provider.is_active)
            .map(|provider| provider.name),
    };
    let default_model = system_config_string(default_model_config.as_ref());
    let enable_usage_tracking = system_config_bool(enable_usage_tracking_config.as_ref(), true);
    let password_policy_level = match system_config_string(password_policy_level_config.as_ref()) {
        Some(value) if matches!(value.as_str(), "weak" | "medium" | "strong") => value,
        _ => "weak".to_string(),
    };

    Ok(build_admin_system_settings_payload_pure(
        default_provider,
        default_model,
        enable_usage_tracking,
        password_policy_level,
    ))
}

pub(crate) async fn apply_admin_system_settings_update(
    state: &AdminAppState<'_>,
    request_body: &Bytes,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    let update = match parse_admin_system_settings_update(request_body) {
        Ok(update) => update,
        Err(err) => return Ok(Err(err)),
    };

    if let Some(default_provider) = update.default_provider {
        if let Some(default_provider) = default_provider {
            let provider_exists = state
                .list_provider_catalog_providers(false)
                .await
                .ok()
                .unwrap_or_default()
                .into_iter()
                .any(|provider| provider.is_active && provider.name == default_provider);
            if !provider_exists {
                return Ok(Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": format!("提供商 '{default_provider}' 不存在或未启用") }),
                )));
            }
            let _ = state
                .upsert_system_config_json_value(
                    "default_provider",
                    &json!(default_provider),
                    Some("系统默认提供商，当用户未设置个人提供商时使用"),
                )
                .await?;
        } else {
            let _ = state
                .upsert_system_config_json_value("default_provider", &serde_json::Value::Null, None)
                .await?;
        }
    }

    if let Some(default_model) = update.default_model {
        let config_value = default_model
            .map(|value| json!(value))
            .unwrap_or(serde_json::Value::Null);
        let _ = state
            .upsert_system_config_json_value("default_model", &config_value, None)
            .await?;
    }

    if let Some(enable_usage_tracking) = update.enable_usage_tracking {
        let _ = state
            .upsert_system_config_json_value(
                "enable_usage_tracking",
                &json!(enable_usage_tracking),
                None,
            )
            .await?;
    }

    if let Some(password_policy_level) = update.password_policy_level {
        let _ = state
            .upsert_system_config_json_value(
                "password_policy_level",
                &json!(password_policy_level),
                None,
            )
            .await?;
    }

    Ok(Ok(build_admin_system_settings_updated_payload()))
}

pub(crate) fn build_admin_api_formats_payload() -> serde_json::Value {
    build_admin_api_formats_payload_pure()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_channel_detects_stable_rc_beta_and_other_prerelease() {
        assert_eq!(update_channel_for_version("v1.2.3"), UpdateChannel::Stable);
        assert_eq!(update_channel_for_version("1.2.3-rc1"), UpdateChannel::Rc);
        assert_eq!(
            update_channel_for_version("1.2.3-beta.2"),
            UpdateChannel::Beta
        );
        assert_eq!(
            update_channel_for_version("1.2.3-alpha.1"),
            UpdateChannel::OtherPrerelease
        );
    }

    #[test]
    fn stable_channel_does_not_allow_prereleases() {
        assert!(!UpdateChannel::Stable.allows_prerelease("v1.2.3-rc1"));
    }

    #[test]
    fn prerelease_channels_only_follow_matching_channel() {
        assert!(UpdateChannel::Rc.allows_prerelease("v1.2.3-rc2"));
        assert!(!UpdateChannel::Rc.allows_prerelease("v1.2.3-beta.1"));
        assert!(UpdateChannel::Beta.allows_prerelease("v1.2.3-beta.2"));
        assert!(!UpdateChannel::Beta.allows_prerelease("v1.2.3-rc1"));
        assert!(UpdateChannel::OtherPrerelease.allows_prerelease("v1.2.3-alpha.1"));
        assert!(UpdateChannel::OtherPrerelease.allows_prerelease("v1.2.3-rc1"));
    }

    #[test]
    fn github_release_tag_url_points_to_explicit_tag_page() {
        assert_eq!(
            github_release_tag_url("v0.7.3"),
            "https://github.com/fawney19/Aether/releases/tag/v0.7.3"
        );
    }

    #[test]
    fn successful_release_cache_is_reused_within_ttl() {
        assert!(should_reuse_releases_cache(
            false,
            false,
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn failed_release_cache_is_not_reused() {
        assert!(!should_reuse_releases_cache(
            false,
            true,
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn force_refresh_bypasses_release_cache() {
        assert!(!should_reuse_releases_cache(
            true,
            false,
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn github_rate_limit_response_is_marked_retryable() {
        let err = github_release_response_error(
            reqwest::StatusCode::FORBIDDEN,
            r#"{"message":"API rate limit exceeded for 1.2.3.4."}"#,
        );
        assert!(err.rate_limited);
        assert!(err.message.contains("GitHub Releases API 已触发限流"));
    }

    #[test]
    fn non_rate_limit_github_response_keeps_http_status_message() {
        let err = github_release_response_error(
            reqwest::StatusCode::FORBIDDEN,
            r#"{"message":"Resource not accessible"}"#,
        );
        assert!(!err.rate_limited);
        assert!(err.message.contains("HTTP 403 Forbidden"));
        assert!(err.message.contains("Resource not accessible"));
    }

    #[test]
    fn self_update_check_update_override_marks_latest_release_non_updatable() {
        let mut payload = json!({
            "has_update": true,
            "updatable": true,
            "update_blocker": serde_json::Value::Null
        });

        apply_self_update_check_update_override_with_blocker(
            &mut payload,
            false,
            SOURCE_BUILD_UPDATE_BLOCKER,
        );

        assert_eq!(payload["updatable"], false);
        assert_eq!(payload["update_blocker"], SOURCE_BUILD_UPDATE_BLOCKER);
    }

    #[test]
    fn self_update_releases_override_marks_non_current_entries_non_updatable() {
        let mut payload = json!({
            "releases": [
                {
                    "version": "v0.7.3",
                    "is_current": false,
                    "updatable": true,
                    "update_blocker": serde_json::Value::Null
                },
                {
                    "version": "v0.7.2",
                    "is_current": true,
                    "updatable": true,
                    "update_blocker": "当前版本"
                }
            ]
        });

        apply_self_update_releases_override_with_blocker(
            &mut payload,
            false,
            SOURCE_BUILD_RELEASE_BLOCKER,
        );

        assert_eq!(payload["releases"][0]["updatable"], false);
        assert_eq!(
            payload["releases"][0]["update_blocker"],
            SOURCE_BUILD_RELEASE_BLOCKER
        );
        assert_eq!(payload["releases"][1]["updatable"], true);
        assert_eq!(payload["releases"][1]["update_blocker"], "当前版本");
    }
}
