use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::config::{
    is_blocked_provider_ip, validate_provider_base_url, RiskControlProviderConfig,
};
use crate::{AppState, GatewayError};

static KEY_FREEZE_UNTIL: LazyLock<DashMap<String, u64>> = LazyLock::new(DashMap::new);
static KEY_HEALTH: LazyLock<DashMap<String, ProviderKeyHealth>> = LazyLock::new(DashMap::new);
static KEY_CURSOR: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Default)]
struct ProviderKeyHealth {
    masked: String,
    failure_count: u64,
    success_count: u64,
    last_error: Option<String>,
    last_checked_at_unix_secs: Option<u64>,
    frozen_until_unix_secs: Option<u64>,
    last_latency_ms: Option<u64>,
    last_http_status: Option<u16>,
    last_tested: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RiskControlProviderKeyStatus {
    pub(crate) index: usize,
    pub(crate) key_hash: String,
    pub(crate) masked: String,
    pub(crate) status: String,
    pub(crate) failure_count: u64,
    pub(crate) success_count: u64,
    pub(crate) last_error: Option<String>,
    pub(crate) last_checked_at_unix_secs: Option<u64>,
    pub(crate) frozen_until_unix_secs: Option<u64>,
    pub(crate) last_latency_ms: Option<u64>,
    pub(crate) last_http_status: Option<u16>,
    pub(crate) last_tested: bool,
    pub(crate) configured: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ModerationOutcome {
    pub(crate) flagged: bool,
    pub(crate) highest_category: Option<String>,
    pub(crate) highest_score: f64,
    pub(crate) category_scores: serde_json::Value,
}

#[derive(Debug, Clone)]
struct ProviderResolvedEndpoint {
    host: String,
    addrs: Vec<SocketAddr>,
}

pub(crate) async fn moderate_text(
    _state: &AppState,
    config: &RiskControlProviderConfig,
    thresholds: &std::collections::BTreeMap<String, f64>,
    input: &str,
) -> Result<ModerationOutcome, GatewayError> {
    if config.api_keys.is_empty() {
        return Err(GatewayError::Internal(
            "risk control moderation api key is not configured".to_string(),
        ));
    }
    validate_provider_base_url(&config.base_url).map_err(GatewayError::Internal)?;
    let endpoint = resolve_and_validate_provider_endpoint(&config.base_url).await?;
    let client = provider_client_for_endpoint(&endpoint)?;
    let attempts = config
        .max_retries
        .saturating_add(1)
        .min(config.api_keys.len().max(1));
    let mut last_error = None;
    for _ in 0..attempts {
        let Some(api_key) = select_api_key(&config.api_keys) else {
            break;
        };
        match moderate_text_once(&client, config, thresholds, input, api_key.as_str()).await {
            Ok(outcome) => {
                mark_key_success(&api_key, outcome.latency_ms, outcome.http_status);
                return Ok(outcome.into_result());
            }
            Err(err) => {
                let frozen_until = err
                    .freeze_key
                    .then(|| freeze_key(&api_key, config.key_freeze_seconds))
                    .flatten();
                mark_key_error(
                    &api_key,
                    format!("{:?}", err.error),
                    err.latency_ms,
                    err.http_status,
                    frozen_until,
                );
                last_error = Some(err.error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        GatewayError::Internal("risk control moderation has no available api key".to_string())
    }))
}

#[derive(Debug)]
struct ModerationAttemptError {
    error: GatewayError,
    freeze_key: bool,
    latency_ms: u64,
    http_status: Option<u16>,
}

#[derive(Debug)]
struct ModerationAttemptSuccess {
    outcome: ModerationOutcome,
    latency_ms: u64,
    http_status: Option<u16>,
}

impl ModerationAttemptSuccess {
    fn into_result(self) -> ModerationOutcome {
        self.outcome
    }
}

async fn moderate_text_once(
    client: &reqwest::Client,
    config: &RiskControlProviderConfig,
    thresholds: &std::collections::BTreeMap<String, f64>,
    input: &str,
    api_key: &str,
) -> Result<ModerationAttemptSuccess, ModerationAttemptError> {
    let started_at = Instant::now();
    let url = format!("{}/v1/moderations", config.base_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .bearer_auth(api_key)
        .timeout(Duration::from_millis(config.timeout_ms))
        .json(&json!({
            "model": config.model,
            "input": input,
        }))
        .send()
        .await
        .map_err(|err| ModerationAttemptError {
            error: GatewayError::Internal(err.to_string()),
            freeze_key: true,
            latency_ms: started_at.elapsed().as_millis() as u64,
            http_status: None,
        })?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| ModerationAttemptError {
            error: GatewayError::Internal(err.to_string()),
            freeze_key: true,
            latency_ms: started_at.elapsed().as_millis() as u64,
            http_status: Some(status.as_u16()),
        })?;
    if !status.is_success() {
        return Err(ModerationAttemptError {
            error: GatewayError::Internal(format!(
                "risk control moderation api returned HTTP {status}"
            )),
            freeze_key: should_freeze_key_for_status(status),
            latency_ms: started_at.elapsed().as_millis() as u64,
            http_status: Some(status.as_u16()),
        });
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|err| ModerationAttemptError {
            error: GatewayError::Internal(format!("invalid moderation response: {err}")),
            freeze_key: false,
            latency_ms: started_at.elapsed().as_millis() as u64,
            http_status: Some(status.as_u16()),
        })?;
    parse_moderation_response(&value, thresholds)
        .map(|outcome| ModerationAttemptSuccess {
            outcome,
            latency_ms: started_at.elapsed().as_millis() as u64,
            http_status: Some(status.as_u16()),
        })
        .map_err(|err| ModerationAttemptError {
            error: err,
            freeze_key: false,
            latency_ms: started_at.elapsed().as_millis() as u64,
            http_status: Some(status.as_u16()),
        })
}

async fn resolve_and_validate_provider_endpoint(
    base_url: &str,
) -> Result<ProviderResolvedEndpoint, GatewayError> {
    let parsed =
        url::Url::parse(base_url).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let host = parsed
        .host_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayError::Internal("Provider Base URL 必须包含 host".to_string()))?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| GatewayError::Internal(format!("Provider Base URL DNS 解析失败: {err}")))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err(GatewayError::Internal(
            "Provider Base URL DNS 未返回可连接地址".to_string(),
        ));
    }
    for target in &addrs {
        if is_blocked_provider_ip(target.ip()) {
            return Err(GatewayError::Internal(
                "Provider Base URL DNS 解析到了内网、回环、链路本地或保留地址".to_string(),
            ));
        }
    }
    Ok(ProviderResolvedEndpoint {
        host: host.to_string(),
        addrs,
    })
}

fn provider_client_for_endpoint(
    endpoint: &ProviderResolvedEndpoint,
) -> Result<reqwest::Client, GatewayError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(endpoint.host.as_str(), endpoint.addrs.as_slice())
        .build()
        .map_err(|err| GatewayError::Internal(format!("Provider HTTP client 初始化失败: {err}")))
}

fn should_freeze_key_for_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED
            | reqwest::StatusCode::FORBIDDEN
            | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

pub(crate) fn parse_moderation_response(
    value: &serde_json::Value,
    thresholds: &std::collections::BTreeMap<String, f64>,
) -> Result<ModerationOutcome, GatewayError> {
    let result = value
        .get("results")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| {
            GatewayError::Internal("moderation response missing results[0]".to_string())
        })?;
    let api_flagged = result
        .get("flagged")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let scores = result
        .get("category_scores")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut highest_category = None;
    let mut highest_score = 0.0;
    if let Some(map) = scores.as_object() {
        for (category, value) in map {
            let Some(score) = value.as_f64().filter(|value| value.is_finite()) else {
                continue;
            };
            if score > highest_score {
                highest_score = score;
                highest_category = Some(category.clone());
            }
        }
    }
    let threshold_flagged = scores.as_object().is_some_and(|map| {
        thresholds.iter().any(|(category, threshold)| {
            map.get(category)
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|score| score >= *threshold)
        })
    });
    Ok(ModerationOutcome {
        flagged: api_flagged || threshold_flagged,
        highest_category,
        highest_score,
        category_scores: scores,
    })
}

fn select_api_key(keys: &[String]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let now = current_unix_secs();
    for _ in 0..keys.len() {
        let index = KEY_CURSOR.fetch_add(1, Ordering::Relaxed) % keys.len();
        let key = keys[index].trim();
        if key.is_empty() {
            continue;
        }
        let key_hash = key_hash(key);
        if KEY_FREEZE_UNTIL
            .get(&key_hash)
            .is_some_and(|until| *until.value() > now)
        {
            continue;
        }
        return Some(key.to_string());
    }
    None
}

pub(crate) fn provider_key_statuses(keys: &[String]) -> Vec<RiskControlProviderKeyStatus> {
    let now = current_unix_secs();
    keys.iter()
        .enumerate()
        .filter_map(|(index, key)| {
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            let key_hash = key_hash(key);
            let masked = mask_key(key);
            let health = KEY_HEALTH.get(&key_hash).map(|value| value.clone());
            let frozen_until = KEY_FREEZE_UNTIL
                .get(&key_hash)
                .map(|value| *value.value())
                .filter(|value| *value > now)
                .or_else(|| health.as_ref().and_then(|item| item.frozen_until_unix_secs));
            let status = match health.as_ref() {
                Some(_) if frozen_until.is_some_and(|value| value > now) => "frozen",
                Some(item) if item.last_error.is_some() => "error",
                Some(item) if item.success_count > 0 || item.last_tested => "ok",
                _ => "unknown",
            };
            Some(RiskControlProviderKeyStatus {
                index,
                key_hash,
                masked: health
                    .as_ref()
                    .map(|item| item.masked.clone())
                    .filter(|value| !value.is_empty())
                    .unwrap_or(masked),
                status: status.to_string(),
                failure_count: health.as_ref().map(|item| item.failure_count).unwrap_or(0),
                success_count: health.as_ref().map(|item| item.success_count).unwrap_or(0),
                last_error: health.as_ref().and_then(|item| item.last_error.clone()),
                last_checked_at_unix_secs: health
                    .as_ref()
                    .and_then(|item| item.last_checked_at_unix_secs),
                frozen_until_unix_secs: frozen_until,
                last_latency_ms: health.as_ref().and_then(|item| item.last_latency_ms),
                last_http_status: health.as_ref().and_then(|item| item.last_http_status),
                last_tested: health
                    .as_ref()
                    .map(|item| item.last_tested)
                    .unwrap_or(false),
                configured: true,
            })
        })
        .collect()
}

fn mark_key_success(api_key: &str, latency_ms: u64, http_status: Option<u16>) {
    let key_hash = key_hash(api_key);
    let now = current_unix_secs();
    KEY_HEALTH
        .entry(key_hash.clone())
        .and_modify(|health| {
            health.masked = mask_key(api_key);
            health.success_count = health.success_count.saturating_add(1);
            health.failure_count = 0;
            health.last_error = None;
            health.last_checked_at_unix_secs = Some(now);
            health.frozen_until_unix_secs = None;
            health.last_latency_ms = Some(latency_ms);
            health.last_http_status = http_status;
            health.last_tested = true;
        })
        .or_insert_with(|| ProviderKeyHealth {
            masked: mask_key(api_key),
            success_count: 1,
            last_checked_at_unix_secs: Some(now),
            last_latency_ms: Some(latency_ms),
            last_http_status: http_status,
            last_tested: true,
            ..ProviderKeyHealth::default()
        });
    KEY_FREEZE_UNTIL.remove(&key_hash);
}

fn mark_key_error(
    api_key: &str,
    error: String,
    latency_ms: u64,
    http_status: Option<u16>,
    frozen_until: Option<u64>,
) {
    let now = current_unix_secs();
    KEY_HEALTH
        .entry(key_hash(api_key))
        .and_modify(|health| {
            health.masked = mask_key(api_key);
            health.failure_count = health.failure_count.saturating_add(1);
            health.last_error = Some(truncate_error(&error));
            health.last_checked_at_unix_secs = Some(now);
            health.frozen_until_unix_secs = frozen_until;
            health.last_latency_ms = Some(latency_ms);
            health.last_http_status = http_status;
            health.last_tested = true;
        })
        .or_insert_with(|| ProviderKeyHealth {
            masked: mask_key(api_key),
            failure_count: 1,
            last_error: Some(truncate_error(&error)),
            last_checked_at_unix_secs: Some(now),
            frozen_until_unix_secs: frozen_until,
            last_latency_ms: Some(latency_ms),
            last_http_status: http_status,
            last_tested: true,
            ..ProviderKeyHealth::default()
        });
}

fn freeze_key(api_key: &str, seconds: u64) -> Option<u64> {
    if seconds == 0 {
        return None;
    }
    let until = current_unix_secs().saturating_add(seconds);
    KEY_FREEZE_UNTIL.insert(key_hash(api_key), until);
    Some(until)
}

fn key_hash(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn mask_key(api_key: &str) -> String {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return String::new();
    }
    let chars = api_key.chars().collect::<Vec<_>>();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let prefix = chars.iter().take(4).collect::<String>();
    let suffix = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}****{suffix}")
}

fn truncate_error(value: &str) -> String {
    value.chars().take(180).collect()
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn threshold_can_flag_even_when_provider_does_not() {
        let thresholds = std::collections::BTreeMap::from([("violence".to_string(), 0.7)]);
        let outcome = parse_moderation_response(
            &json!({
                "results": [{
                    "flagged": false,
                    "category_scores": {"violence": 0.8, "hate": 0.1}
                }]
            }),
            &thresholds,
        )
        .expect("response should parse");

        assert!(outcome.flagged);
        assert_eq!(outcome.highest_category.as_deref(), Some("violence"));
    }

    #[test]
    fn only_freezes_keys_for_auth_rate_limit_network_or_server_statuses() {
        assert!(should_freeze_key_for_status(
            reqwest::StatusCode::UNAUTHORIZED
        ));
        assert!(should_freeze_key_for_status(reqwest::StatusCode::FORBIDDEN));
        assert!(should_freeze_key_for_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(should_freeze_key_for_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(!should_freeze_key_for_status(
            reqwest::StatusCode::BAD_REQUEST
        ));
        assert!(!should_freeze_key_for_status(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY
        ));
    }

    #[test]
    fn provider_key_status_reports_frozen_error_and_unknown_states() {
        let frozen_key = "key-frozen-123456";
        let ok_key = "key-ok-123456";
        let unknown_key = "key-unknown-123456";
        let until = freeze_key(frozen_key, 60).expect("should freeze");
        mark_key_error(
            frozen_key,
            "rate limited".to_string(),
            12,
            Some(reqwest::StatusCode::TOO_MANY_REQUESTS.as_u16()),
            Some(until),
        );
        mark_key_success(ok_key, 7, Some(reqwest::StatusCode::OK.as_u16()));

        let statuses = provider_key_statuses(&[
            frozen_key.to_string(),
            ok_key.to_string(),
            unknown_key.to_string(),
        ]);

        assert_eq!(statuses[0].status, "frozen");
        assert_eq!(statuses[1].status, "ok");
        assert_eq!(statuses[2].status, "unknown");
    }
}
