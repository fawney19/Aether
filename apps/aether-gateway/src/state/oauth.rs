use super::{
    provider_transport_snapshot_looks_refreshed, AppState, CachedProviderTransportSnapshot,
    GatewayError, ProviderTransportSnapshotCacheKey, ProviderTransportSnapshotFlight,
    ProviderTransportSnapshotFlightResult, PROVIDER_TRANSPORT_SNAPSHOT_CACHE_MAX_ENTRIES,
    PROVIDER_TRANSPORT_SNAPSHOT_CACHE_STALE_TTL, PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL,
};
use crate::handlers::shared::default_provider_key_status_snapshot;
use crate::provider_transport::LocalOAuthHttpExecutor;

use super::super::provider_transport;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_contracts::{
    ExecutionPlan, ExecutionTimeouts, ProxySnapshot, RequestBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyStatusSnapshotUpdate, StoredProviderCatalogKey,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use dashmap::{mapref::entry::Entry as DashMapEntry, DashMap};
use flate2::read::{DeflateDecoder, GzDecoder};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_crypto::encrypt_python_fernet_plaintext;

const LOCAL_OAUTH_HTTP_TIMEOUT_MS: u64 = 30_000;
const OAUTH_ACCOUNT_BLOCK_PREFIX: &str = "[ACCOUNT_BLOCK] ";
const OAUTH_EXPIRED_PREFIX: &str = "[OAUTH_EXPIRED] ";
const OAUTH_REFRESH_FAILED_PREFIX: &str = "[REFRESH_FAILED] ";
const OAUTH_REQUEST_FAILED_PREFIX: &str = "[REQUEST_FAILED] ";

struct GatewayLocalOAuthHttpExecutor<'a> {
    state: &'a AppState,
}

enum ProviderTransportSnapshotCacheLookup {
    Fresh(Arc<provider_transport::GatewayProviderTransportSnapshot>),
    Stale(Arc<provider_transport::GatewayProviderTransportSnapshot>),
    Miss,
}

enum ProviderTransportSnapshotReloadResult {
    Published(Arc<provider_transport::GatewayProviderTransportSnapshot>),
    Missing,
    Invalidated,
}

enum ProviderTransportSnapshotInflightRegistration {
    Leader(ProviderTransportSnapshotInflightGuard),
    Follower(Arc<ProviderTransportSnapshotFlight>),
    Retry,
}

struct ProviderTransportSnapshotInflightGuard {
    inflight: Arc<DashMap<ProviderTransportSnapshotCacheKey, Arc<ProviderTransportSnapshotFlight>>>,
    cache_key: Option<ProviderTransportSnapshotCacheKey>,
    flight: Arc<ProviderTransportSnapshotFlight>,
}

impl ProviderTransportSnapshotInflightGuard {
    fn generation(&self) -> u64 {
        self.flight.generation()
    }

    fn generation_is_current(&self, state: &AppState) -> bool {
        state
            .provider_transport_snapshot_cache_generation
            .load(Ordering::Acquire)
            == self.generation()
    }

    fn finish(&mut self, result: ProviderTransportSnapshotFlightResult) {
        let Some(cache_key) = self.cache_key.take() else {
            return;
        };
        // Publish completion before exposing a vacant map entry. Requests in
        // this small window join the completed flight instead of issuing a
        // duplicate reload for a missing/error result.
        self.flight.complete(result);
        self.inflight
            .remove_if(&cache_key, |_, current| Arc::ptr_eq(current, &self.flight));
    }
}

impl Drop for ProviderTransportSnapshotInflightGuard {
    fn drop(&mut self) {
        // Cancellation must release the key and wake every follower. One of
        // them can then claim leadership and retry the interrupted load.
        self.finish(ProviderTransportSnapshotFlightResult::Retry);
    }
}

fn provider_transport_snapshot_flight_result(
    result: &Result<ProviderTransportSnapshotReloadResult, GatewayError>,
) -> ProviderTransportSnapshotFlightResult {
    match result {
        Ok(ProviderTransportSnapshotReloadResult::Published(snapshot)) => {
            ProviderTransportSnapshotFlightResult::Published(Arc::clone(snapshot))
        }
        Ok(ProviderTransportSnapshotReloadResult::Missing) => {
            ProviderTransportSnapshotFlightResult::Missing
        }
        Ok(ProviderTransportSnapshotReloadResult::Invalidated) => {
            ProviderTransportSnapshotFlightResult::Invalidated
        }
        Err(err) => ProviderTransportSnapshotFlightResult::Error(err.clone()),
    }
}

fn trimmed_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn tagged_reason(reason: Option<&str>, prefix: &str) -> Option<String> {
    reason.and_then(|value| {
        value
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn oauth_access_token_expired(expires_at_unix_secs: Option<u64>, now_unix_secs: u64) -> bool {
    expires_at_unix_secs.is_none_or(|expires_at| expires_at == 0 || expires_at <= now_unix_secs)
}

fn local_oauth_refresh_entry_should_stay_memory_only(
    transport: &provider_transport::GatewayProviderTransportSnapshot,
    entry: &provider_transport::CachedOAuthEntry,
) -> bool {
    entry
        .provider_type
        .trim()
        .eq_ignore_ascii_case(provider_transport::vertex::VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE)
        && provider_transport::is_vertex_service_account_transport_context(transport)
}

fn oauth_auth_config_refresh_token_fingerprint(auth_config: Option<&str>) -> Option<String> {
    let parsed = auth_config
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())?;
    oauth_metadata_refresh_token_fingerprint(Some(&parsed))
}

fn oauth_metadata_refresh_token_fingerprint(metadata: Option<&Value>) -> Option<String> {
    metadata
        .and_then(Value::as_object)
        .and_then(|object| object.get("refresh_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(secret_fingerprint)
}

fn local_oauth_request_refresh_token_fingerprint(
    request: &provider_transport::LocalOAuthHttpRequest,
) -> (Option<String>, Option<usize>) {
    if let Some(json_body) = request.json_body.as_ref() {
        return json_body
            .as_object()
            .and_then(|object| object.get("refresh_token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| (Some(secret_fingerprint(value)), Some(value.len())))
            .unwrap_or((None, None));
    }

    let Some(body_bytes) = request.body_bytes.as_ref() else {
        return (None, None);
    };
    for (key, value) in url::form_urlencoded::parse(body_bytes) {
        if key == "refresh_token" {
            let value = value.trim();
            if !value.is_empty() {
                return (Some(secret_fingerprint(value)), Some(value.len()));
            }
        }
    }
    (None, None)
}

fn local_oauth_log_excerpt(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "-".to_string();
    }
    body.chars().take(300).collect()
}

fn local_oauth_proxy_is_tunnel(proxy: Option<&ProxySnapshot>) -> bool {
    let Some(proxy) = proxy else {
        return false;
    };
    if proxy.enabled == Some(false) {
        return false;
    }
    proxy
        .mode
        .as_deref()
        .map(str::trim)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("tunnel"))
}

fn local_oauth_proxy_extra_string<'a>(
    proxy: Option<&'a ProxySnapshot>,
    key: &str,
) -> Option<&'a str> {
    proxy?
        .extra
        .as_ref()
        .and_then(|extra| extra.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn secret_fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut fingerprint = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(&mut fingerprint, "{byte:02x}");
    }
    fingerprint
}

fn oauth_invalid_reason_is_account_block(reason: Option<&str>) -> bool {
    let Some(reason) = reason.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX) {
        return true;
    }
    let snapshot =
        aether_admin::provider::status::resolve_account_status_snapshot(None, None, Some(reason));
    snapshot.blocked
        && !matches!(
            snapshot.code.trim().to_ascii_lowercase().as_str(),
            "oauth_token_invalid"
                | "oauth_token_expired"
                | "oauth_expired"
                | "oauth_refresh_failed"
        )
}

fn normalize_local_oauth_refresh_error_message(
    status_code: Option<u16>,
    body_excerpt: Option<&str>,
) -> String {
    let mut message = None::<String>;
    let mut error_code = None::<String>;
    let mut error_type = None::<String>;

    if let Some(body_excerpt) = body_excerpt {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body_excerpt) {
            if let Some(object) = value.as_object() {
                if let Some(error_object) =
                    object.get("error").and_then(serde_json::Value::as_object)
                {
                    message = error_object
                        .get("message")
                        .or_else(|| error_object.get("error_description"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                    error_code = error_object
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                    error_type = error_object
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
                if message.is_none() {
                    message = object
                        .get("message")
                        .or_else(|| object.get("error_description"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                }
                if error_code.is_none() {
                    error_code = object
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
                if error_type.is_none() {
                    error_type = object
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
            }
        }
    }

    let message = message
        .or_else(|| {
            body_excerpt
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(300).collect::<String>())
        })
        .unwrap_or_default();
    let lowered = message.to_ascii_lowercase();
    let error_code = error_code.unwrap_or_default();
    let error_type = error_type.unwrap_or_default();

    if error_code == "refresh_token_reused"
        || lowered.contains("already been used to generate a new access token")
    {
        return "refresh_token 已被使用并轮换，请重新登录授权".to_string();
    }
    if error_code == "invalid_grant"
        || error_code == "invalid_refresh_token"
        || error_code == "refresh_token_expired"
        || lowered.contains("could not validate your refresh token")
        || (lowered.contains("refresh token")
            && ["expired", "revoked", "invalid"]
                .iter()
                .any(|keyword| lowered.contains(keyword)))
    {
        return "refresh_token 无效、已过期或已撤销，请重新登录授权".to_string();
    }
    if error_type == "invalid_request_error" && !message.is_empty() {
        return message;
    }
    if !message.is_empty() {
        return message;
    }
    status_code
        .map(|status_code| format!("HTTP {status_code}"))
        .unwrap_or_else(|| "未知错误".to_string())
}

fn merge_local_oauth_refresh_failure_reason(
    current_reason: Option<&str>,
    refresh_reason: &str,
) -> Option<String> {
    let current_reason = current_reason.map(str::trim).unwrap_or_default();
    let refresh_reason = refresh_reason.trim();
    if refresh_reason.is_empty() {
        return (!current_reason.is_empty()).then(|| current_reason.to_string());
    }
    if current_reason.is_empty() {
        return Some(refresh_reason.to_string());
    }
    if current_reason.starts_with(OAUTH_EXPIRED_PREFIX) {
        if refresh_reason.starts_with(OAUTH_REFRESH_FAILED_PREFIX)
            && !current_reason
                .lines()
                .map(str::trim)
                .any(|line| line.starts_with(OAUTH_REFRESH_FAILED_PREFIX))
        {
            return Some(format!("{current_reason}\n{refresh_reason}"));
        }
        return Some(current_reason.to_string());
    }
    if oauth_invalid_reason_is_account_block(Some(current_reason)) {
        return None;
    }
    Some(refresh_reason.to_string())
}

fn local_oauth_refresh_success_invalid_state(
    key: &StoredProviderCatalogKey,
) -> (Option<u64>, Option<String>) {
    let current_reason = key
        .oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if oauth_invalid_reason_is_account_block(Some(current_reason)) {
        return (
            key.oauth_invalid_at_unix_secs,
            Some(current_reason.to_string()),
        );
    }
    (None, None)
}

fn default_oauth_status_snapshot_value() -> Value {
    default_provider_key_status_snapshot()
        .get("oauth")
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "code": "none",
                "label": Value::Null,
                "reason": Value::Null,
                "expires_at": Value::Null,
                "invalid_at": Value::Null,
                "source": Value::Null,
                "requires_reauth": false,
                "expiring_soon": false,
            })
        })
}

fn build_oauth_status_snapshot_value(key: &StoredProviderCatalogKey) -> Value {
    if !key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return default_oauth_status_snapshot_value();
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let expires_at_unix_secs = key.expires_at_unix_secs;
    let invalid_at_unix_secs = key.oauth_invalid_at_unix_secs;
    let invalid_reason = trimmed_reason(key.oauth_invalid_reason.as_deref());

    if let Some(reason) = tagged_reason(invalid_reason.as_deref(), OAUTH_EXPIRED_PREFIX) {
        let (code, label) =
            aether_admin::provider::status::oauth_token_snapshot_status_parts(reason.as_str());
        return json!({
            "code": code,
            "label": label,
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_invalid",
            "requires_reauth": code == "invalid",
            "expiring_soon": false,
        });
    }
    if let Some(reason) = tagged_reason(invalid_reason.as_deref(), OAUTH_REFRESH_FAILED_PREFIX) {
        let access_token_expired = oauth_access_token_expired(expires_at_unix_secs, now_unix_secs);
        return json!({
            "code": if access_token_expired { "invalid" } else { "reauth_required" },
            "label": if access_token_expired { "已失效" } else { "续期失败" },
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_refresh",
            "requires_reauth": true,
            "usable_until_expiry": !access_token_expired,
            "expiring_soon": false,
        });
    }
    if let Some(reason) = tagged_reason(invalid_reason.as_deref(), OAUTH_REQUEST_FAILED_PREFIX) {
        return json!({
            "code": "check_failed",
            "label": "检查失败",
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": Value::Null,
            "source": "oauth_request",
            "requires_reauth": false,
            "expiring_soon": false,
        });
    }
    if invalid_reason
        .as_deref()
        .is_some_and(|reason| !reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX))
        || invalid_at_unix_secs.is_some()
    {
        return json!({
            "code": "invalid",
            "label": "已失效",
            "reason": invalid_reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_invalid",
            "requires_reauth": true,
            "expiring_soon": false,
        });
    }

    let Some(expires_at_unix_secs) = expires_at_unix_secs else {
        return default_oauth_status_snapshot_value();
    };
    if expires_at_unix_secs <= now_unix_secs {
        return json!({
            "code": "expired",
            "label": "已过期",
            "reason": "Access Token 已过期，等待自动续期",
            "expires_at": expires_at_unix_secs,
            "invalid_at": Value::Null,
            "source": "expires_at",
            "requires_reauth": false,
            "expiring_soon": false,
        });
    }

    let expiring_soon = expires_at_unix_secs.saturating_sub(now_unix_secs) < 24 * 60 * 60;
    json!({
        "code": if expiring_soon { "expiring" } else { "valid" },
        "label": if expiring_soon { "即将过期" } else { "有效" },
        "reason": Value::Null,
        "expires_at": expires_at_unix_secs,
        "invalid_at": Value::Null,
        "source": "expires_at",
        "requires_reauth": false,
        "expiring_soon": expiring_soon,
    })
}

fn sync_provider_key_oauth_status_snapshot(
    status_snapshot: Option<Value>,
    key: &StoredProviderCatalogKey,
) -> Option<Value> {
    let mut snapshot = status_snapshot
        .and_then(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .or_else(|| default_provider_key_status_snapshot().as_object().cloned())
        .unwrap_or_default();
    snapshot.insert("oauth".to_string(), build_oauth_status_snapshot_value(key));
    Some(Value::Object(snapshot))
}

#[async_trait::async_trait]
impl<'a> provider_transport::LocalOAuthHttpExecutor for GatewayLocalOAuthHttpExecutor<'a> {
    async fn execute(
        &self,
        provider_type: &'static str,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
        request: &provider_transport::LocalOAuthHttpRequest,
    ) -> Result<
        provider_transport::LocalOAuthHttpResponse,
        provider_transport::LocalOAuthRefreshError,
    > {
        self.state
            .execute_local_oauth_http_request(provider_type, transport, request)
            .await
    }
}

impl AppState {
    pub(crate) fn clear_provider_transport_snapshot_cache(&self) {
        self.provider_transport_snapshot_cache_generation
            .fetch_add(1, Ordering::AcqRel);
        self.provider_transport_snapshot_cache.clear();

        // Keep a concurrently-created flight from the new generation. Every
        // older flight is completed as invalidated so its followers retry
        // immediately instead of waiting for the old database read to finish.
        let mut invalidated = Vec::new();
        self.provider_transport_snapshot_inflight
            .retain(|_, flight| {
                let current_generation = self
                    .provider_transport_snapshot_cache_generation
                    .load(Ordering::Acquire);
                if flight.generation() < current_generation {
                    invalidated.push(Arc::clone(flight));
                    false
                } else {
                    true
                }
            });
        for flight in invalidated {
            flight.complete(ProviderTransportSnapshotFlightResult::Invalidated);
        }
    }

    fn register_provider_transport_snapshot_inflight(
        &self,
        cache_key: &ProviderTransportSnapshotCacheKey,
        generation: u64,
    ) -> ProviderTransportSnapshotInflightRegistration {
        let flight = Arc::new(ProviderTransportSnapshotFlight::new(generation));
        match self
            .provider_transport_snapshot_inflight
            .entry(cache_key.clone())
        {
            DashMapEntry::Occupied(entry) => {
                let current = Arc::clone(entry.get());
                if current.generation() == generation {
                    return ProviderTransportSnapshotInflightRegistration::Follower(current);
                }

                // A caller that observed an older generation must never evict
                // a newer flight. If this caller is current, the occupied
                // entry is left over from a clear that has not retained its
                // shard yet and can be invalidated here.
                if self
                    .provider_transport_snapshot_cache_generation
                    .load(Ordering::Acquire)
                    != generation
                {
                    return ProviderTransportSnapshotInflightRegistration::Retry;
                }
                let invalidated = entry.remove();
                invalidated.complete(ProviderTransportSnapshotFlightResult::Invalidated);
                ProviderTransportSnapshotInflightRegistration::Retry
            }
            DashMapEntry::Vacant(entry) => {
                if self
                    .provider_transport_snapshot_cache_generation
                    .load(Ordering::Acquire)
                    != generation
                {
                    return ProviderTransportSnapshotInflightRegistration::Retry;
                }
                entry.insert(Arc::clone(&flight));
                ProviderTransportSnapshotInflightRegistration::Leader(
                    ProviderTransportSnapshotInflightGuard {
                        inflight: Arc::clone(&self.provider_transport_snapshot_inflight),
                        cache_key: Some(cache_key.clone()),
                        flight,
                    },
                )
            }
        }
    }

    fn get_cached_provider_transport_snapshot_arc(
        &self,
        cache_key: &ProviderTransportSnapshotCacheKey,
    ) -> ProviderTransportSnapshotCacheLookup {
        let cached = self
            .provider_transport_snapshot_cache
            .get(cache_key)
            .map(|entry| entry.clone());
        let Some(cached) = cached else {
            return ProviderTransportSnapshotCacheLookup::Miss;
        };
        if cached.generation
            != self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire)
        {
            self.provider_transport_snapshot_cache
                .remove_if(cache_key, |_, current| {
                    current.generation == cached.generation
                });
            return ProviderTransportSnapshotCacheLookup::Miss;
        }
        let age = cached.loaded_at.elapsed();
        if age <= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL {
            return ProviderTransportSnapshotCacheLookup::Fresh(cached.snapshot);
        }
        if age <= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_STALE_TTL {
            return ProviderTransportSnapshotCacheLookup::Stale(cached.snapshot);
        }
        if self
            .provider_transport_snapshot_cache
            .get(cache_key)
            .is_some_and(|entry| {
                entry.loaded_at.elapsed() > PROVIDER_TRANSPORT_SNAPSHOT_CACHE_STALE_TTL
            })
        {
            self.provider_transport_snapshot_cache
                .remove_if(cache_key, |_, current| {
                    current.generation == cached.generation
                });
        }
        ProviderTransportSnapshotCacheLookup::Miss
    }

    fn put_cached_provider_transport_snapshot(
        &self,
        cache_key: ProviderTransportSnapshotCacheKey,
        snapshot: Arc<provider_transport::GatewayProviderTransportSnapshot>,
        generation: u64,
    ) -> bool {
        if generation
            != self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire)
        {
            return false;
        }
        if self.provider_transport_snapshot_cache.len()
            >= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_MAX_ENTRIES
        {
            self.provider_transport_snapshot_cache.retain(|_, entry| {
                entry.loaded_at.elapsed() <= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_STALE_TTL
            });
            if self.provider_transport_snapshot_cache.len()
                >= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_MAX_ENTRIES
            {
                let oldest_key = self
                    .provider_transport_snapshot_cache
                    .iter()
                    .min_by_key(|entry| entry.value().loaded_at)
                    .map(|entry| entry.key().clone());
                if let Some(oldest_key) = oldest_key {
                    self.provider_transport_snapshot_cache.remove(&oldest_key);
                }
            }
        }
        self.provider_transport_snapshot_cache.insert(
            cache_key.clone(),
            CachedProviderTransportSnapshot {
                loaded_at: std::time::Instant::now(),
                generation,
                snapshot,
            },
        );
        if generation
            != self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire)
        {
            self.provider_transport_snapshot_cache
                .remove_if(&cache_key, |_, current| current.generation == generation);
            return false;
        }
        true
    }

    async fn reload_provider_transport_snapshot(
        &self,
        cache_key: &ProviderTransportSnapshotCacheKey,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        generation: u64,
    ) -> Result<ProviderTransportSnapshotReloadResult, GatewayError> {
        if generation
            != self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire)
        {
            return Ok(ProviderTransportSnapshotReloadResult::Invalidated);
        }

        let loaded = self
            .read_provider_transport_snapshot_uncached(provider_id, endpoint_id, key_id)
            .await?;
        if generation
            != self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire)
        {
            return Ok(ProviderTransportSnapshotReloadResult::Invalidated);
        }

        let Some(snapshot) = loaded else {
            return Ok(ProviderTransportSnapshotReloadResult::Missing);
        };
        let snapshot = self.apply_global_format_conversion_override(snapshot).await;
        if generation
            != self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire)
        {
            return Ok(ProviderTransportSnapshotReloadResult::Invalidated);
        }

        let snapshot = Arc::new(snapshot);
        if self.put_cached_provider_transport_snapshot(
            cache_key.clone(),
            Arc::clone(&snapshot),
            generation,
        ) {
            Ok(ProviderTransportSnapshotReloadResult::Published(snapshot))
        } else {
            Ok(ProviderTransportSnapshotReloadResult::Invalidated)
        }
    }

    fn start_provider_transport_snapshot_background_refresh(
        &self,
        cache_key: ProviderTransportSnapshotCacheKey,
        provider_id: String,
        endpoint_id: String,
        key_id: String,
    ) {
        let mut inflight_guard = loop {
            let generation = self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire);
            match self.register_provider_transport_snapshot_inflight(&cache_key, generation) {
                ProviderTransportSnapshotInflightRegistration::Leader(guard) => break guard,
                ProviderTransportSnapshotInflightRegistration::Follower(_) => return,
                ProviderTransportSnapshotInflightRegistration::Retry => continue,
            }
        };
        let generation = inflight_guard.generation();
        let state = self.clone();
        tokio::spawn(async move {
            let result = state
                .reload_provider_transport_snapshot(
                    &cache_key,
                    &provider_id,
                    &endpoint_id,
                    &key_id,
                    generation,
                )
                .await;
            if matches!(&result, Ok(ProviderTransportSnapshotReloadResult::Missing))
                && state
                    .provider_transport_snapshot_cache_generation
                    .load(Ordering::Acquire)
                    == generation
            {
                state
                    .provider_transport_snapshot_cache
                    .remove_if(&cache_key, |_, current| current.generation == generation);
            }
            let flight_result = if inflight_guard.generation_is_current(&state) {
                provider_transport_snapshot_flight_result(&result)
            } else {
                ProviderTransportSnapshotFlightResult::Invalidated
            };
            inflight_guard.finish(flight_result);
        });
    }

    pub(crate) async fn read_provider_transport_snapshot_uncached(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<crate::provider_transport::GatewayProviderTransportSnapshot>, GatewayError>
    {
        self.data
            .read_provider_transport_snapshot(provider_id, endpoint_id, key_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    async fn apply_global_format_conversion_override(
        &self,
        mut snapshot: crate::provider_transport::GatewayProviderTransportSnapshot,
    ) -> crate::provider_transport::GatewayProviderTransportSnapshot {
        let global_config =
            Box::pin(self.read_system_config_json_value("enable_format_conversion"))
                .await
                .ok()
                .flatten();
        let global_enabled = global_config
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if global_enabled {
            snapshot.provider.enable_format_conversion = true;
        }
        snapshot
    }

    pub(crate) async fn list_enabled_oauth_module_providers(
        &self,
    ) -> Result<
        Vec<aether_data::repository::auth_modules::StoredOAuthProviderModuleConfig>,
        GatewayError,
    > {
        self.data
            .list_enabled_oauth_module_providers()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_ldap_module_config(
        &self,
    ) -> Result<Option<aether_data::repository::auth_modules::StoredLdapModuleConfig>, GatewayError>
    {
        self.data
            .get_ldap_module_config()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn upsert_ldap_module_config(
        &self,
        config: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
    ) -> Result<Option<aether_data::repository::auth_modules::StoredLdapModuleConfig>, GatewayError>
    {
        self.data
            .upsert_ldap_module_config(config)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_active_local_admin_users_with_valid_password()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_oauth_provider_configs(
        &self,
    ) -> Result<
        Vec<aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
        GatewayError,
    > {
        self.data
            .list_oauth_provider_configs()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<
        Option<aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
        GatewayError,
    > {
        self.data
            .get_oauth_provider_config(provider_type)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_locked_users_if_oauth_provider_disabled(
        &self,
        provider_type: &str,
        ldap_exclusive: bool,
    ) -> Result<usize, GatewayError> {
        self.data
            .count_locked_users_if_oauth_provider_disabled(provider_type, ldap_exclusive)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn upsert_oauth_provider_config(
        &self,
        record: &aether_data::repository::oauth_providers::UpsertOAuthProviderConfigRecord,
    ) -> Result<
        Option<aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
        GatewayError,
    > {
        self.data
            .upsert_oauth_provider_config(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_oauth_provider_config(provider_type)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) fn encryption_key(&self) -> Option<&str> {
        self.data.encryption_key()
    }

    pub(crate) fn has_auth_module_writer(&self) -> bool {
        self.data.has_auth_module_writer()
    }

    pub(crate) fn provider_oauth_token_url(
        &self,
        _provider_type: &str,
        default_token_url: &str,
    ) -> String {
        #[cfg(test)]
        {
            if let Some(value) = self
                .provider_oauth_token_url_overrides
                .lock()
                .expect("provider oauth token url overrides should lock")
                .get(_provider_type.trim())
                .cloned()
            {
                return value;
            }
        }

        default_token_url.to_string()
    }

    pub(crate) fn save_provider_oauth_state_for_tests(&self, _key: &str, _value: &str) -> bool {
        #[cfg(test)]
        {
            if let Some(store) = self.provider_oauth_state_store.as_ref() {
                store
                    .lock()
                    .expect("provider oauth state store should lock")
                    .insert(_key.to_string(), _value.to_string());
                return true;
            }
        }

        false
    }

    pub(crate) fn take_provider_oauth_state_for_tests(&self, _key: &str) -> Option<String> {
        #[cfg(test)]
        {
            return self.provider_oauth_state_store.as_ref().and_then(|store| {
                store
                    .lock()
                    .expect("provider oauth state store should lock")
                    .remove(_key)
            });
        }

        #[allow(unreachable_code)]
        None
    }

    pub(crate) fn save_provider_oauth_device_session_for_tests(
        &self,
        _key: &str,
        _value: &str,
    ) -> bool {
        #[cfg(test)]
        {
            if let Some(store) = self.provider_oauth_device_session_store.as_ref() {
                store
                    .lock()
                    .expect("provider oauth device session store should lock")
                    .insert(_key.to_string(), _value.to_string());
                return true;
            }
        }

        false
    }

    pub(crate) fn load_provider_oauth_device_session_for_tests(
        &self,
        _key: &str,
    ) -> Option<String> {
        #[cfg(test)]
        {
            return self
                .provider_oauth_device_session_store
                .as_ref()
                .and_then(|store| {
                    store
                        .lock()
                        .expect("provider oauth device session store should lock")
                        .get(_key)
                        .cloned()
                });
        }

        #[allow(unreachable_code)]
        None
    }

    pub(crate) fn save_provider_oauth_batch_task_for_tests(
        &self,
        _key: &str,
        _value: &str,
    ) -> bool {
        #[cfg(test)]
        {
            if let Some(store) = self.provider_oauth_batch_task_store.as_ref() {
                store
                    .lock()
                    .expect("provider oauth batch task store should lock")
                    .insert(_key.to_string(), _value.to_string());
                return true;
            }
        }

        false
    }

    pub(crate) fn load_provider_oauth_batch_task_for_tests(&self, _key: &str) -> Option<String> {
        #[cfg(test)]
        {
            return self
                .provider_oauth_batch_task_store
                .as_ref()
                .and_then(|store| {
                    store
                        .lock()
                        .expect("provider oauth batch task store should lock")
                        .get(_key)
                        .cloned()
                });
        }

        #[allow(unreachable_code)]
        None
    }

    pub(crate) async fn read_provider_transport_snapshot_arc(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<
        Option<Arc<crate::provider_transport::GatewayProviderTransportSnapshot>>,
        GatewayError,
    > {
        let Some(cache_key) =
            ProviderTransportSnapshotCacheKey::new(provider_id, endpoint_id, key_id)
        else {
            return Ok(self
                .read_provider_transport_snapshot_uncached(provider_id, endpoint_id, key_id)
                .await?
                .map(Arc::new));
        };
        loop {
            match self.get_cached_provider_transport_snapshot_arc(&cache_key) {
                ProviderTransportSnapshotCacheLookup::Fresh(snapshot) => {
                    return Ok(Some(snapshot));
                }
                ProviderTransportSnapshotCacheLookup::Stale(snapshot) => {
                    self.start_provider_transport_snapshot_background_refresh(
                        cache_key.clone(),
                        provider_id.to_string(),
                        endpoint_id.to_string(),
                        key_id.to_string(),
                    );
                    return Ok(Some(snapshot));
                }
                ProviderTransportSnapshotCacheLookup::Miss => {}
            }

            let generation = self
                .provider_transport_snapshot_cache_generation
                .load(Ordering::Acquire);
            match self.register_provider_transport_snapshot_inflight(&cache_key, generation) {
                ProviderTransportSnapshotInflightRegistration::Retry => continue,
                ProviderTransportSnapshotInflightRegistration::Follower(flight) => {
                    let flight_generation = flight.generation();
                    let result = flight.wait().await;
                    if self
                        .provider_transport_snapshot_cache_generation
                        .load(Ordering::Acquire)
                        != flight_generation
                    {
                        continue;
                    }
                    match result {
                        ProviderTransportSnapshotFlightResult::Published(snapshot) => {
                            return Ok(Some(snapshot));
                        }
                        ProviderTransportSnapshotFlightResult::Missing => return Ok(None),
                        ProviderTransportSnapshotFlightResult::Error(err) => return Err(err),
                        ProviderTransportSnapshotFlightResult::Invalidated
                        | ProviderTransportSnapshotFlightResult::Retry => continue,
                    }
                }
                ProviderTransportSnapshotInflightRegistration::Leader(mut inflight_guard) => {
                    if !inflight_guard.generation_is_current(self) {
                        inflight_guard.finish(ProviderTransportSnapshotFlightResult::Invalidated);
                        continue;
                    }

                    // A different flight may have published between the first
                    // cache check and this registration. Recheck before doing
                    // the only database reload for this flight.
                    if let ProviderTransportSnapshotCacheLookup::Fresh(snapshot) =
                        self.get_cached_provider_transport_snapshot_arc(&cache_key)
                    {
                        if !inflight_guard.generation_is_current(self) {
                            inflight_guard
                                .finish(ProviderTransportSnapshotFlightResult::Invalidated);
                            continue;
                        }
                        inflight_guard.finish(ProviderTransportSnapshotFlightResult::Published(
                            Arc::clone(&snapshot),
                        ));
                        if !inflight_guard.generation_is_current(self) {
                            continue;
                        }
                        return Ok(Some(snapshot));
                    }

                    let result = self
                        .reload_provider_transport_snapshot(
                            &cache_key,
                            provider_id,
                            endpoint_id,
                            key_id,
                            generation,
                        )
                        .await;
                    let flight_result = if inflight_guard.generation_is_current(self) {
                        provider_transport_snapshot_flight_result(&result)
                    } else {
                        ProviderTransportSnapshotFlightResult::Invalidated
                    };
                    inflight_guard.finish(flight_result);
                    if !inflight_guard.generation_is_current(self) {
                        continue;
                    }
                    match result {
                        Ok(ProviderTransportSnapshotReloadResult::Published(snapshot)) => {
                            return Ok(Some(snapshot));
                        }
                        Ok(ProviderTransportSnapshotReloadResult::Missing) => return Ok(None),
                        Ok(ProviderTransportSnapshotReloadResult::Invalidated) => continue,
                        Err(err) => return Err(err),
                    }
                }
            }
        }
    }

    pub(crate) async fn read_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<crate::provider_transport::GatewayProviderTransportSnapshot>, GatewayError>
    {
        Ok(self
            .read_provider_transport_snapshot_arc(provider_id, endpoint_id, key_id)
            .await?
            .map(|snapshot| (*snapshot).clone()))
    }

    pub(crate) async fn update_provider_catalog_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, GatewayError> {
        let updated = self
            .data
            .update_provider_catalog_key_oauth_credentials(
                key_id,
                encrypted_api_key,
                encrypted_auth_config,
                expires_at_unix_secs,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(updated)
    }

    pub(crate) async fn update_provider_catalog_key_oauth_runtime_state(
        &self,
        key_id: &str,
        oauth_invalid_at_unix_secs: Option<u64>,
        oauth_invalid_reason: Option<&str>,
        encrypted_auth_config_update: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, GatewayError> {
        let updated = self
            .data
            .update_provider_catalog_key_oauth_runtime_state(
                key_id,
                oauth_invalid_at_unix_secs,
                oauth_invalid_reason,
                encrypted_auth_config_update,
                updated_at_unix_secs,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(updated)
    }

    pub(crate) async fn resolve_local_oauth_request_auth(
        &self,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
    ) -> Result<Option<provider_transport::LocalResolvedOAuthRequestAuth>, GatewayError> {
        let distributed_lock = self.runtime_state.as_ref();
        let lock_owner = format!("aether-gateway-{}", std::process::id());
        let mut current_transport = transport.clone();
        let executor = GatewayLocalOAuthHttpExecutor { state: self };

        for _ in 0..2 {
            let resolution = match self
                .oauth_refresh
                .resolve_with_result(
                    &executor,
                    &current_transport,
                    Some(distributed_lock),
                    Some(lock_owner.as_str()),
                )
                .await
            {
                Ok(resolution) => resolution,
                Err(provider_transport::LocalOAuthRefreshError::HttpStatus {
                    status_code,
                    body_excerpt,
                    ..
                }) if matches!(status_code, 400 | 401 | 403) => {
                    if let Err(err) = self
                        .persist_local_oauth_refresh_failure_state(
                            &current_transport,
                            status_code,
                            body_excerpt.as_str(),
                            false,
                        )
                        .await
                    {
                        tracing::warn!(
                            key_id = %current_transport.key.id,
                            provider_type = %current_transport.provider.provider_type,
                            error = ?err,
                            "gateway local oauth refresh failure persistence failed"
                        );
                    }
                    return Ok(None);
                }
                Err(err) => return Err(GatewayError::Internal(err.to_string())),
            };

            if resolution
                .as_ref()
                .is_some_and(|resolution| resolution.refresh_in_flight)
            {
                let Some(reloaded_transport) = self
                    .wait_for_remote_oauth_refresh(&current_transport)
                    .await?
                else {
                    continue;
                };
                current_transport = reloaded_transport;
                continue;
            }

            if let Some(refreshed_entry) = resolution
                .as_ref()
                .and_then(|resolution| resolution.refreshed_entry.as_ref())
            {
                if let Err(err) = self
                    .persist_local_oauth_refresh_entry(&current_transport, refreshed_entry)
                    .await
                {
                    tracing::warn!(
                        key_id = %current_transport.key.id,
                        provider_type = %current_transport.provider.provider_type,
                        error = ?err,
                        "gateway local oauth refresh persistence failed"
                    );
                    let _ = self
                        .invalidate_local_oauth_refresh_entry(&current_transport.key.id)
                        .await;
                } else {
                    self.oauth_refresh
                        .store_cached_entry(
                            current_transport.key.id.trim(),
                            refreshed_entry.clone(),
                        )
                        .await;
                }
            }

            return Ok(resolution.and_then(|resolution| resolution.auth));
        }

        Ok(None)
    }

    pub(crate) async fn force_local_oauth_refresh_entry(
        &self,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
    ) -> Result<
        Option<provider_transport::CachedOAuthEntry>,
        provider_transport::LocalOAuthRefreshError,
    > {
        let distributed_lock = self.runtime_state.as_ref();
        let lock_owner = format!("aether-gateway-admin-{}", std::process::id());
        let mut current_transport = transport.clone();
        current_transport.key.decrypted_api_key = "__placeholder__".to_string();
        let executor = GatewayLocalOAuthHttpExecutor { state: self };
        let transport_refresh_token_fingerprint = oauth_auth_config_refresh_token_fingerprint(
            current_transport.key.decrypted_auth_config.as_deref(),
        )
        .unwrap_or_else(|| "-".to_string());
        tracing::info!(
            key_id = %current_transport.key.id,
            provider_id = %current_transport.provider.id,
            provider_type = %current_transport.provider.provider_type,
            transport_refresh_token_fingerprint = %transport_refresh_token_fingerprint,
            has_transport_auth_config = current_transport
                .key
                .decrypted_auth_config
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            "gateway manual oauth refresh starting"
        );

        for _ in 0..2 {
            let resolution = self
                .oauth_refresh
                .force_refresh_with_result(
                    &executor,
                    &current_transport,
                    Some(distributed_lock),
                    Some(lock_owner.as_str()),
                )
                .await?;

            if resolution
                .as_ref()
                .is_some_and(|resolution| resolution.refresh_in_flight)
            {
                let Some(reloaded_transport) = self
                    .wait_for_remote_oauth_refresh(&current_transport)
                    .await
                    .map_err(
                        |err| provider_transport::LocalOAuthRefreshError::InvalidResponse {
                            provider_type: "gateway",
                            message: format!("{err:?}"),
                        },
                    )?
                else {
                    continue;
                };
                current_transport = reloaded_transport;
                current_transport.key.decrypted_api_key = "__placeholder__".to_string();
                continue;
            }

            if let Some(refreshed_entry) = resolution
                .as_ref()
                .and_then(|resolution| resolution.refreshed_entry.as_ref())
            {
                if let Err(err) = self
                    .persist_local_oauth_refresh_entry(&current_transport, refreshed_entry)
                    .await
                {
                    tracing::warn!(
                        key_id = %current_transport.key.id,
                        provider_type = %current_transport.provider.provider_type,
                        error = ?err,
                        "gateway manual oauth refresh persistence failed"
                    );
                    let _ = self
                        .invalidate_local_oauth_refresh_entry(&current_transport.key.id)
                        .await;
                    return Err(
                        provider_transport::LocalOAuthRefreshError::InvalidResponse {
                            provider_type: "gateway",
                            message: format!("local oauth refresh persistence failed: {err:?}"),
                        },
                    );
                }
                self.oauth_refresh
                    .store_cached_entry(current_transport.key.id.trim(), refreshed_entry.clone())
                    .await;
                return Ok(Some(refreshed_entry.clone()));
            }

            return Ok(None);
        }

        Ok(None)
    }

    pub(crate) async fn invalidate_local_oauth_refresh_entry(&self, key_id: &str) -> bool {
        self.oauth_refresh.invalidate_cached_entry(key_id).await
    }

    pub(crate) async fn mark_provider_catalog_key_oauth_invalid(
        &self,
        key_id: &str,
        provider_type: &str,
        invalid_reason: &str,
    ) -> Result<bool, GatewayError> {
        let invalid_reason = invalid_reason.trim();
        if invalid_reason.is_empty() {
            return Ok(false);
        }

        let Some(mut latest_key) = self
            .data
            .list_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };

        if !provider_key_is_oauth_managed(&latest_key, provider_type) {
            return Ok(false);
        }

        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let (oauth_invalid_at_unix_secs, oauth_invalid_reason) = merge_runtime_oauth_invalid_state(
            provider_type,
            &latest_key,
            invalid_reason,
            now_unix_secs,
        );
        if oauth_invalid_at_unix_secs == latest_key.oauth_invalid_at_unix_secs
            && oauth_invalid_reason == latest_key.oauth_invalid_reason
        {
            return Ok(false);
        }

        latest_key.oauth_invalid_at_unix_secs = oauth_invalid_at_unix_secs;
        latest_key.oauth_invalid_reason = oauth_invalid_reason;
        latest_key.updated_at_unix_secs = Some(now_unix_secs);
        let current_status_snapshot = latest_key.status_snapshot.take();
        latest_key.status_snapshot =
            sync_provider_key_oauth_status_snapshot(current_status_snapshot, &latest_key);
        let mut updated = self
            .update_provider_catalog_key_oauth_runtime_state(
                key_id,
                latest_key.oauth_invalid_at_unix_secs,
                latest_key.oauth_invalid_reason.as_deref(),
                None,
                latest_key.updated_at_unix_secs,
            )
            .await?;
        if updated {
            updated = self
                .update_provider_catalog_key_status_snapshot(
                    &provider_key_oauth_status_snapshot_update(&latest_key),
                )
                .await?;
            self.clear_provider_transport_snapshot_cache();
            let _ = self.invalidate_local_oauth_refresh_entry(key_id).await;
        }
        Ok(updated)
    }

    async fn persist_local_oauth_refresh_entry(
        &self,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
        entry: &provider_transport::CachedOAuthEntry,
    ) -> Result<(), GatewayError> {
        let key_id = transport.key.id.trim();
        if key_id.is_empty() {
            return Ok(());
        }

        if local_oauth_refresh_entry_should_stay_memory_only(transport, entry) {
            tracing::info!(
                key_id = %key_id,
                provider_id = %transport.provider.id,
                provider_type = %transport.provider.provider_type,
                expires_at_unix_secs = ?entry.expires_at_unix_secs,
                "gateway local oauth refresh entry kept in memory only"
            );
            return Ok(());
        }

        let Some(encryption_key) = self.data.encryption_key() else {
            return Ok(());
        };

        let access_token = entry
            .auth_header_value
            .trim()
            .strip_prefix("Bearer ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                GatewayError::Internal(
                    "local oauth refresh produced non-bearer auth header".to_string(),
                )
            })?;

        let encrypted_api_key = encrypt_python_fernet_plaintext(encryption_key, access_token)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let encrypted_auth_config = entry
            .metadata
            .as_ref()
            .map(|value| serde_json::to_string(value))
            .transpose()
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .map(|value| encrypt_python_fernet_plaintext(encryption_key, value.as_str()))
            .transpose()
            .map_err(|err| GatewayError::Internal(err.to_string()))?;

        let Some(mut latest_key) = self
            .data
            .list_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .into_iter()
            .next()
        else {
            return Ok(());
        };

        latest_key.encrypted_api_key = Some(encrypted_api_key.clone());
        latest_key.encrypted_auth_config = encrypted_auth_config.clone();
        latest_key.expires_at_unix_secs = entry.expires_at_unix_secs;
        let (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
            local_oauth_refresh_success_invalid_state(&latest_key);
        latest_key.oauth_invalid_at_unix_secs = oauth_invalid_at_unix_secs;
        latest_key.oauth_invalid_reason = oauth_invalid_reason;
        latest_key.updated_at_unix_secs = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
        );
        let current_status_snapshot = latest_key.status_snapshot.take();
        latest_key.status_snapshot =
            sync_provider_key_oauth_status_snapshot(current_status_snapshot, &latest_key);
        let mut updated = self
            .update_provider_catalog_key_oauth_credentials(
                key_id,
                &encrypted_api_key,
                encrypted_auth_config.as_deref(),
                entry.expires_at_unix_secs,
            )
            .await?;
        if updated {
            updated = self
                .update_provider_catalog_key_oauth_runtime_state(
                    key_id,
                    latest_key.oauth_invalid_at_unix_secs,
                    latest_key.oauth_invalid_reason.as_deref(),
                    None,
                    latest_key.updated_at_unix_secs,
                )
                .await?;
        }
        if updated {
            updated = self
                .update_provider_catalog_key_status_snapshot(
                    &provider_key_oauth_status_snapshot_update(&latest_key),
                )
                .await?;
            self.clear_provider_transport_snapshot_cache();
        }
        let metadata_refresh_token_fingerprint =
            oauth_metadata_refresh_token_fingerprint(entry.metadata.as_ref())
                .unwrap_or_else(|| "-".to_string());
        tracing::info!(
            key_id = %key_id,
            provider_id = %transport.provider.id,
            provider_type = %transport.provider.provider_type,
            updated,
            metadata_has_refresh_token = entry
                .metadata
                .as_ref()
                .and_then(|value| value.as_object())
                .and_then(|object| object.get("refresh_token"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            metadata_refresh_token_fingerprint = %metadata_refresh_token_fingerprint,
            expires_at_unix_secs = ?entry.expires_at_unix_secs,
            cleared_provider_transport_snapshot_cache = updated,
            "gateway local oauth refresh entry persisted"
        );
        Ok(())
    }

    pub(crate) async fn persist_local_oauth_refresh_failure_state(
        &self,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
        status_code: u16,
        body_excerpt: &str,
        access_token_invalid_proven: bool,
    ) -> Result<bool, GatewayError> {
        let key_id = transport.key.id.trim();
        if key_id.is_empty() {
            return Ok(false);
        }

        let Some(mut latest_key) = self
            .data
            .list_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };

        if !provider_key_is_oauth_managed(&latest_key, transport.provider.provider_type.as_str()) {
            return Ok(false);
        }

        let refresh_reason = format!(
            "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 ({status_code}): {}",
            normalize_local_oauth_refresh_error_message(Some(status_code), Some(body_excerpt))
        );
        let Some(merged_reason) = merge_local_oauth_refresh_failure_reason(
            latest_key.oauth_invalid_reason.as_deref(),
            &refresh_reason,
        ) else {
            return Ok(false);
        };

        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let mut updated = false;
        if latest_key.oauth_invalid_reason.as_deref() != Some(merged_reason.as_str())
            || latest_key.oauth_invalid_at_unix_secs.is_none()
        {
            latest_key.oauth_invalid_at_unix_secs = latest_key
                .oauth_invalid_at_unix_secs
                .or(Some(now_unix_secs));
            latest_key.oauth_invalid_reason = Some(merged_reason);
            latest_key.updated_at_unix_secs = Some(now_unix_secs);
            let current_status_snapshot = latest_key.status_snapshot.take();
            latest_key.status_snapshot =
                sync_provider_key_oauth_status_snapshot(current_status_snapshot, &latest_key);

            updated = self
                .update_provider_catalog_key_oauth_runtime_state(
                    key_id,
                    latest_key.oauth_invalid_at_unix_secs,
                    latest_key.oauth_invalid_reason.as_deref(),
                    None,
                    latest_key.updated_at_unix_secs,
                )
                .await?;
            if updated {
                updated = self
                    .update_provider_catalog_key_status_snapshot(
                        &provider_key_oauth_status_snapshot_update(&latest_key),
                    )
                    .await?;
                self.clear_provider_transport_snapshot_cache();
                let _ = self.invalidate_local_oauth_refresh_entry(key_id).await;
            }
        }

        let auto_removed = if admin_provider_quota_pure::provider_auto_remove_banned_keys(
            transport.provider.config.as_ref(),
        ) && admin_provider_quota_pure::should_auto_remove_oauth_invalid_key(
            &latest_key,
            None,
            access_token_invalid_proven,
            now_unix_secs,
        ) {
            self.clear_provider_transport_snapshot_cache();
            if self.delete_provider_catalog_key(key_id).await? {
                let deleted_key_ids = [key_id.to_string()];
                self.cleanup_deleted_provider_catalog_refs(
                    &transport.provider.id,
                    false,
                    &[],
                    &deleted_key_ids,
                )
                .await?;
                let _ = self.invalidate_local_oauth_refresh_entry(key_id).await;
                true
            } else {
                false
            }
        } else {
            false
        };
        tracing::info!(
            key_id = %key_id,
            provider_id = %transport.provider.id,
            provider_type = %transport.provider.provider_type,
            status_code,
            updated,
            auto_removed,
            cleared_provider_transport_snapshot_cache = updated || auto_removed,
            "gateway local oauth refresh failure state persisted"
        );
        Ok(auto_removed)
    }

    async fn execute_local_oauth_http_request(
        &self,
        provider_type: &'static str,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
        request: &provider_transport::LocalOAuthHttpRequest,
    ) -> Result<
        provider_transport::LocalOAuthHttpResponse,
        provider_transport::LocalOAuthRefreshError,
    > {
        if local_oauth_request_uses_direct_client(request.url.as_str()) {
            let executor =
                provider_transport::ReqwestLocalOAuthHttpExecutor::new(self.client.clone());
            return executor.execute(provider_type, transport, request).await;
        }

        let body = if let Some(json_body) = request.json_body.clone() {
            RequestBody::from_json(json_body)
        } else {
            RequestBody {
                json_body: None,
                body_bytes_b64: request
                    .body_bytes
                    .as_ref()
                    .map(|bytes| STANDARD.encode(bytes)),
                body_ref: None,
            }
        };
        let proxy_snapshot = self
            .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
            .await;
        let proxy_is_tunnel = local_oauth_proxy_is_tunnel(proxy_snapshot.as_ref());
        let mut headers = request.headers.clone();
        headers.insert(
            EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string(),
            "true".to_string(),
        );
        if proxy_is_tunnel {
            headers.insert(
                EXECUTION_REQUEST_HTTP1_ONLY_HEADER.to_string(),
                "true".to_string(),
            );
        }
        let plan = ExecutionPlan {
            request_id: request.request_id.to_string(),
            candidate_id: None,
            provider_name: Some(transport.provider.name.clone()),
            provider_id: transport.provider.id.clone(),
            endpoint_id: transport.endpoint.id.clone(),
            key_id: transport.key.id.clone(),
            method: request.method.as_str().to_string(),
            url: request.url.clone(),
            headers,
            content_type: request
                .headers
                .get("content-type")
                .map(String::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            content_encoding: None,
            body,
            stream: false,
            client_api_format: "provider_oauth:local_refresh".to_string(),
            provider_api_format: "provider_oauth:local_refresh".to_string(),
            model_name: Some(provider_type.to_string()),
            proxy: proxy_snapshot,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                read_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                write_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                pool_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                total_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                ..ExecutionTimeouts::default()
            }),
        };
        let (request_refresh_token_fingerprint, request_refresh_token_len) =
            local_oauth_request_refresh_token_fingerprint(request);
        tracing::info!(
            key_id = %transport.key.id,
            provider_id = %transport.provider.id,
            endpoint_id = %transport.endpoint.id,
            provider_type,
            request_id = %request.request_id,
            method = %plan.method,
            token_url = %plan.url,
            content_type = plan.content_type.as_deref().unwrap_or("-"),
            body_bytes_len = ?request.body_bytes.as_ref().map(Vec::len),
            json_body_present = request.json_body.is_some(),
            request_refresh_token_fingerprint = request_refresh_token_fingerprint
                .as_deref()
                .unwrap_or("-"),
            request_refresh_token_len = ?request_refresh_token_len,
            proxy_node_id = ?plan.proxy.as_ref().and_then(|proxy| proxy.node_id.as_deref()),
            proxy_mode = plan.proxy.as_ref().and_then(|proxy| proxy.mode.as_deref()).unwrap_or("-"),
            proxy_enabled = ?plan.proxy.as_ref().and_then(|proxy| proxy.enabled),
            proxy_url_present = plan
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.url.as_deref())
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            proxy_is_tunnel,
            tunnel_base_url_present = local_oauth_proxy_extra_string(
                plan.proxy.as_ref(),
                "tunnel_base_url"
            )
            .is_some(),
            tunnel_owner_instance_id = local_oauth_proxy_extra_string(
                plan.proxy.as_ref(),
                "tunnel_owner_instance_id"
            )
            .unwrap_or("-"),
            follow_redirects = plan
                .headers
                .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                .map(String::as_str)
                .unwrap_or("-"),
            http1_only = plan
                .headers
                .get(EXECUTION_REQUEST_HTTP1_ONLY_HEADER)
                .map(String::as_str)
                .unwrap_or("-"),
            "gateway local oauth execution request prepared"
        );
        let result =
            crate::execution_runtime::execute_execution_runtime_sync_plan(self, None, &plan)
                .await
                .map_err(
                    |err| provider_transport::LocalOAuthRefreshError::InvalidResponse {
                        provider_type,
                        message: err.into_message(),
                    },
                )?;
        let response_body_text = local_oauth_execution_body_text(&result);
        if (200..300).contains(&result.status_code) {
            tracing::info!(
                key_id = %transport.key.id,
                provider_id = %transport.provider.id,
                endpoint_id = %transport.endpoint.id,
                provider_type,
                request_id = %request.request_id,
                status_code = result.status_code,
                request_refresh_token_fingerprint = request_refresh_token_fingerprint
                    .as_deref()
                    .unwrap_or("-"),
                "gateway local oauth execution response received"
            );
        } else {
            tracing::warn!(
                key_id = %transport.key.id,
                provider_id = %transport.provider.id,
                endpoint_id = %transport.endpoint.id,
                provider_type,
                request_id = %request.request_id,
                status_code = result.status_code,
                request_refresh_token_fingerprint = request_refresh_token_fingerprint
                    .as_deref()
                    .unwrap_or("-"),
                body_excerpt = %local_oauth_log_excerpt(response_body_text.as_str()),
                "gateway local oauth execution response returned error"
            );
        }
        Ok(provider_transport::LocalOAuthHttpResponse {
            status_code: result.status_code,
            body_text: response_body_text,
        })
    }

    async fn wait_for_remote_oauth_refresh(
        &self,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
    ) -> Result<Option<provider_transport::GatewayProviderTransportSnapshot>, GatewayError> {
        if !self.data.has_provider_catalog_reader() {
            return Ok(None);
        }

        for _ in 0..20 {
            let Some(reloaded_transport) = self
                .read_provider_transport_snapshot_uncached(
                    &transport.provider.id,
                    &transport.endpoint.id,
                    &transport.key.id,
                )
                .await?
            else {
                return Ok(None);
            };

            if provider_transport_snapshot_looks_refreshed(transport, &reloaded_transport) {
                return Ok(Some(reloaded_transport));
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(None)
    }
}

fn provider_key_oauth_status_snapshot_update(
    key: &StoredProviderCatalogKey,
) -> ProviderCatalogKeyStatusSnapshotUpdate {
    let oauth = key
        .status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("oauth"))
        .cloned()
        .unwrap_or(Value::Null);
    ProviderCatalogKeyStatusSnapshotUpdate {
        key_id: key.id.clone(),
        status_snapshot_patch: json!({"oauth":oauth}),
        updated_at_unix_secs: key.updated_at_unix_secs,
    }
}

fn merge_runtime_oauth_invalid_state(
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    invalid_reason: &str,
    now_unix_secs: u64,
) -> (Option<u64>, Option<String>) {
    let candidate_reason = invalid_reason.trim();
    if candidate_reason.is_empty() {
        return (
            key.oauth_invalid_at_unix_secs,
            key.oauth_invalid_reason.clone(),
        );
    }

    if provider_type.trim().eq_ignore_ascii_case("codex") {
        return admin_provider_quota_pure::codex_build_invalid_state(
            key,
            candidate_reason.to_string(),
            now_unix_secs,
        );
    }

    let current_reason = key
        .oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if current_reason == candidate_reason {
        return (
            key.oauth_invalid_at_unix_secs,
            (!current_reason.is_empty()).then_some(current_reason.to_string()),
        );
    }

    (Some(now_unix_secs), Some(candidate_reason.to_string()))
}

fn local_oauth_execution_body_text(result: &aether_contracts::ExecutionResult) -> String {
    result
        .body
        .as_ref()
        .and_then(|body| local_oauth_execution_body_bytes(&result.headers, body))
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(|value| serde_json::to_string(value).ok())
        })
        .unwrap_or_default()
}

fn local_oauth_execution_body_bytes(
    headers: &BTreeMap<String, String>,
    body: &aether_contracts::ResponseBody,
) -> Option<Vec<u8>> {
    let bytes = body
        .body_bytes_b64
        .as_deref()
        .and_then(|value| STANDARD.decode(value).ok())?;
    let encoding = headers
        .get("content-encoding")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    match encoding.as_deref() {
        Some("gzip") => {
            let mut decoder = GzDecoder::new(bytes.as_slice());
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        Some("deflate") => {
            let mut decoder = DeflateDecoder::new(bytes.as_slice());
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        _ => Some(bytes),
    }
}

fn local_oauth_request_uses_direct_client(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .map(|addr| addr.is_loopback())
                    .unwrap_or(false)
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogKeyListQuery, ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
        StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
    };
    use aether_data_contracts::DataLayerError;
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::Notify;

    use super::{
        AppState, ProviderTransportSnapshotCacheKey, ProviderTransportSnapshotFlight,
        ProviderTransportSnapshotFlightResult, ProviderTransportSnapshotInflightRegistration,
        PROVIDER_TRANSPORT_SNAPSHOT_CACHE_STALE_TTL, PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL,
    };
    use crate::data::GatewayDataState;

    fn sample_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "provider-1".to_string(),
            Some("https://provider.example".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(true, false, false, None, None, None, None, None, None)
    }

    fn sample_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.provider.example".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:chat"])),
            "plain-upstream-key".to_string(),
            None,
            None,
            Some(json!({"openai:chat": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    fn state_with_global_format_conversion(enabled: bool) -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![sample_key()],
        ));
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            repository,
            "test-encryption-key",
        )
        .with_system_config_values_for_tests(vec![(
            "enable_format_conversion".to_string(),
            json!(enabled),
        )]);
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state)
    }

    struct BlockingProviderCatalogReadRepository {
        inner: Arc<InMemoryProviderCatalogReadRepository>,
        key_reads: AtomicUsize,
        block_on_key_read: usize,
        blocked_key_read_started: Notify,
        release_blocked_key_read: Notify,
    }

    impl BlockingProviderCatalogReadRepository {
        fn new(inner: Arc<InMemoryProviderCatalogReadRepository>) -> Self {
            Self::blocking_on_key_read(inner, 1)
        }

        fn blocking_on_key_read(
            inner: Arc<InMemoryProviderCatalogReadRepository>,
            block_on_key_read: usize,
        ) -> Self {
            Self {
                inner,
                key_reads: AtomicUsize::new(0),
                block_on_key_read,
                blocked_key_read_started: Notify::new(),
                release_blocked_key_read: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl ProviderCatalogReadRepository for BlockingProviderCatalogReadRepository {
        async fn list_providers(
            &self,
            active_only: bool,
        ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
            self.inner.list_providers(active_only).await
        }

        async fn list_providers_by_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
            self.inner.list_providers_by_ids(provider_ids).await
        }

        async fn list_endpoints_by_ids(
            &self,
            endpoint_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
            self.inner.list_endpoints_by_ids(endpoint_ids).await
        }

        async fn list_endpoints_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
            self.inner
                .list_endpoints_by_provider_ids(provider_ids)
                .await
        }

        async fn list_keys_by_ids(
            &self,
            key_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
            let keys = self.inner.list_keys_by_ids(key_ids).await?;
            let read_number = self.key_reads.fetch_add(1, Ordering::AcqRel) + 1;
            if read_number == self.block_on_key_read {
                self.blocked_key_read_started.notify_one();
                self.release_blocked_key_read.notified().await;
            }
            Ok(keys)
        }

        async fn list_keys_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
            self.inner.list_keys_by_provider_ids(provider_ids).await
        }

        async fn list_key_summaries_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
            self.inner
                .list_key_summaries_by_provider_ids(provider_ids)
                .await
        }

        async fn list_key_maintenance_summaries_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
            self.inner
                .list_key_maintenance_summaries_by_provider_ids(provider_ids)
                .await
        }

        async fn list_keys_page(
            &self,
            query: &ProviderCatalogKeyListQuery,
        ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
            self.inner.list_keys_page(query).await
        }

        async fn list_key_stats_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
            self.inner
                .list_key_stats_by_provider_ids(provider_ids)
                .await
        }
    }

    #[tokio::test]
    async fn global_format_conversion_overrides_snapshot_without_persisting_provider_value() {
        let state = state_with_global_format_conversion(false);

        let snapshot = state
            .read_provider_transport_snapshot("provider-1", "endpoint-1", "key-1")
            .await
            .expect("snapshot read should succeed")
            .expect("snapshot should exist");
        assert!(!snapshot.provider.enable_format_conversion);

        state
            .upsert_system_config_json_value("enable_format_conversion", &json!(true), None)
            .await
            .expect("global config update should succeed");
        let snapshot = state
            .read_provider_transport_snapshot("provider-1", "endpoint-1", "key-1")
            .await
            .expect("snapshot read should succeed")
            .expect("snapshot should exist");
        assert!(snapshot.provider.enable_format_conversion);

        state
            .upsert_system_config_json_value("enable_format_conversion", &json!(false), None)
            .await
            .expect("global config update should succeed");
        let snapshot = state
            .read_provider_transport_snapshot("provider-1", "endpoint-1", "key-1")
            .await
            .expect("snapshot read should succeed")
            .expect("snapshot should exist");
        assert!(!snapshot.provider.enable_format_conversion);
    }

    #[tokio::test]
    async fn provider_transport_snapshot_inflight_entry_is_removed_after_read() {
        let state = state_with_global_format_conversion(false);

        let snapshot = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("snapshot read should succeed");

        assert!(snapshot.is_some());
        assert!(state.provider_transport_snapshot_inflight.is_empty());
    }

    #[tokio::test]
    async fn finished_transport_flight_detects_generation_clear_before_return() {
        let state = state_with_global_format_conversion(false);
        let snapshot = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("snapshot read should succeed")
            .expect("snapshot should exist");
        state.clear_provider_transport_snapshot_cache();

        let cache_key = ProviderTransportSnapshotCacheKey::new("provider-1", "endpoint-1", "key-1")
            .expect("cache key should build");
        let generation = state
            .provider_transport_snapshot_cache_generation
            .load(Ordering::Acquire);
        let mut guard =
            match state.register_provider_transport_snapshot_inflight(&cache_key, generation) {
                ProviderTransportSnapshotInflightRegistration::Leader(guard) => guard,
                _ => panic!("empty current-generation key should register a leader"),
            };
        guard.finish(ProviderTransportSnapshotFlightResult::Published(snapshot));
        assert!(guard.generation_is_current(&state));

        state.clear_provider_transport_snapshot_cache();
        assert!(!guard.generation_is_current(&state));
    }

    #[tokio::test]
    async fn transport_cache_clear_retries_inflight_load_before_publishing_snapshot() {
        let inner = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![sample_key()],
        ));
        let reader = Arc::new(BlockingProviderCatalogReadRepository::new(Arc::clone(
            &inner,
        )));
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            reader.clone(),
            "test-encryption-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let read_state = state.clone();
        let read_task = tokio::spawn(async move {
            read_state
                .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                .await
                .expect("transport snapshot read should succeed")
                .expect("transport snapshot should exist")
        });
        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("first transport load should reach the blocked key read");

        let mut inactive_key = sample_key();
        inactive_key.is_active = false;
        inner
            .update_key(&inactive_key)
            .await
            .expect("provider key should update while the old snapshot load is blocked");
        state.clear_provider_transport_snapshot_cache();
        reader.release_blocked_key_read.notify_one();

        let snapshot = tokio::time::timeout(Duration::from_secs(1), read_task)
            .await
            .expect("transport load should retry promptly after invalidation")
            .expect("transport load task should join");
        assert!(!snapshot.key.is_active);
        assert!(reader.key_reads.load(Ordering::Acquire) >= 2);

        let cached = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("cached transport read should succeed")
            .expect("cached transport should exist");
        assert!(!cached.key.is_active);
        assert!(Arc::ptr_eq(&snapshot, &cached));
    }

    fn blocking_transport_state(
        block_on_key_read: usize,
    ) -> (
        AppState,
        Arc<InMemoryProviderCatalogReadRepository>,
        Arc<BlockingProviderCatalogReadRepository>,
    ) {
        let inner = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![sample_key()],
        ));
        let reader = Arc::new(BlockingProviderCatalogReadRepository::blocking_on_key_read(
            Arc::clone(&inner),
            block_on_key_read,
        ));
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            reader.clone(),
            "test-encryption-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        (state, inner, reader)
    }

    fn age_transport_snapshot_cache(state: &AppState, age: Duration) {
        let cache_key = ProviderTransportSnapshotCacheKey::new("provider-1", "endpoint-1", "key-1")
            .expect("cache key should build");
        let mut cached = state
            .provider_transport_snapshot_cache
            .get_mut(&cache_key)
            .expect("snapshot should be cached before aging");
        cached.loaded_at = Instant::now() - age;
    }

    async fn wait_for_transport_refresh_to_finish(state: &AppState) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state.provider_transport_snapshot_inflight.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background transport refresh should finish");
    }

    async fn wait_for_transport_flight_followers(
        state: &AppState,
        expected_strong_count: usize,
    ) -> Arc<ProviderTransportSnapshotFlight> {
        let cache_key = ProviderTransportSnapshotCacheKey::new("provider-1", "endpoint-1", "key-1")
            .expect("cache key should build");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(flight) = state
                    .provider_transport_snapshot_inflight
                    .get(&cache_key)
                    .filter(|flight| Arc::strong_count(flight.value()) >= expected_strong_count)
                    .map(|flight| flight.clone())
                {
                    return flight;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all transport snapshot followers should register")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cold_transport_snapshot_broadcasts_to_twenty_thousand_followers() {
        const REQUESTS: usize = 20_000;
        let (state, _inner, reader) = blocking_transport_state(1);
        let mut reads = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let read_state = state.clone();
            reads.push(tokio::spawn(async move {
                read_state
                    .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                    .await
                    .expect("transport snapshot read should succeed")
                    .expect("transport snapshot should exist")
            }));
        }

        tokio::time::timeout(
            Duration::from_secs(2),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("leader should reach the database barrier");
        let _flight = wait_for_transport_flight_followers(&state, REQUESTS + 1).await;
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 1);

        reader.release_blocked_key_read.notify_one();
        let snapshots = tokio::time::timeout(Duration::from_secs(5), async {
            let mut snapshots = Vec::with_capacity(REQUESTS);
            for read in reads {
                snapshots.push(read.await.expect("transport read task should join"));
            }
            snapshots
        })
        .await
        .expect("all followers should wake from one broadcast");
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 1);
        let first = snapshots
            .first()
            .expect("at least one snapshot should exist");
        for snapshot in snapshots.iter().skip(1) {
            assert!(Arc::ptr_eq(first, snapshot));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn transport_cache_clear_wakes_old_followers_before_old_leader_finishes() {
        const REQUESTS: usize = 64;
        let (state, inner, reader) = blocking_transport_state(1);
        let mut reads = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let read_state = state.clone();
            reads.push(tokio::spawn(async move {
                read_state
                    .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                    .await
                    .expect("transport snapshot read should succeed")
                    .expect("transport snapshot should exist")
            }));
        }
        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("old leader should reach the database barrier");
        let _old_flight = wait_for_transport_flight_followers(&state, REQUESTS + 1).await;

        let mut inactive_key = sample_key();
        inactive_key.is_active = false;
        inner
            .update_key(&inactive_key)
            .await
            .expect("provider key should update before invalidation");
        state.clear_provider_transport_snapshot_cache();

        // The old leader is still blocked. A follower must nevertheless claim
        // the new generation and perform the replacement read immediately.
        tokio::time::timeout(Duration::from_secs(1), async {
            while reader.key_reads.load(Ordering::Acquire) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("new generation follower should reload before old leader release");

        reader.release_blocked_key_read.notify_one();
        let snapshots = tokio::time::timeout(Duration::from_secs(3), async {
            let mut snapshots = Vec::with_capacity(REQUESTS);
            for read in reads {
                snapshots.push(read.await.expect("transport read task should join"));
            }
            snapshots
        })
        .await
        .expect("invalidated followers should complete");
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 2);
        assert!(snapshots.iter().all(|snapshot| !snapshot.key.is_active));
    }

    #[tokio::test]
    async fn cancelled_transport_snapshot_leader_releases_followers() {
        let (state, _inner, reader) = blocking_transport_state(1);
        let leader_state = state.clone();
        let leader = tokio::spawn(async move {
            leader_state
                .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                .await
        });
        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("leader should reach the database barrier");

        let follower_state = state.clone();
        let follower = tokio::spawn(async move {
            follower_state
                .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                .await
                .expect("replacement transport read should succeed")
                .expect("replacement snapshot should exist")
        });
        let _old_flight = wait_for_transport_flight_followers(&state, 3).await;
        leader.abort();
        assert!(leader
            .await
            .expect_err("leader should be cancelled")
            .is_cancelled());

        let replacement = tokio::time::timeout(Duration::from_secs(2), follower)
            .await
            .expect("follower should be released after leader cancellation")
            .expect("follower task should join");
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 2);
        assert!(replacement.key.is_active);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn missing_transport_snapshot_result_is_broadcast_to_all_followers() {
        const REQUESTS: usize = 64;
        let inner = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![sample_endpoint()],
            Vec::new(),
        ));
        let reader = Arc::new(BlockingProviderCatalogReadRepository::new(Arc::clone(
            &inner,
        )));
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            reader.clone(),
            "test-encryption-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let mut reads = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let read_state = state.clone();
            reads.push(tokio::spawn(async move {
                read_state
                    .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                    .await
                    .expect("missing transport read should not fail")
            }));
        }
        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("missing-result leader should reach the database barrier");
        let _flight = wait_for_transport_flight_followers(&state, REQUESTS + 1).await;

        reader.release_blocked_key_read.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            for read in reads {
                assert!(read.await.expect("missing read task should join").is_none());
            }
        })
        .await
        .expect("missing result should wake all followers");
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn transport_snapshot_error_is_broadcast_to_all_followers() {
        const REQUESTS: usize = 64;
        let mut mismatched_key = sample_key();
        mismatched_key.provider_id = "provider-other".to_string();
        let inner = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![sample_endpoint()],
            vec![mismatched_key],
        ));
        let reader = Arc::new(BlockingProviderCatalogReadRepository::new(Arc::clone(
            &inner,
        )));
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            reader.clone(),
            "test-encryption-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let mut reads = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let read_state = state.clone();
            reads.push(tokio::spawn(async move {
                read_state
                    .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                    .await
                    .expect_err("provider mismatch should fail")
                    .into_message()
            }));
        }
        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("error-result leader should reach the database barrier");
        let _flight = wait_for_transport_flight_followers(&state, REQUESTS + 1).await;

        reader.release_blocked_key_read.notify_one();
        let messages = tokio::time::timeout(Duration::from_secs(2), async {
            let mut messages = Vec::with_capacity(REQUESTS);
            for read in reads {
                messages.push(read.await.expect("error read task should join"));
            }
            messages
        })
        .await
        .expect("error result should wake all followers");
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 1);
        assert!(messages
            .iter()
            .all(|message| message.contains("provider_api_keys.provider_id mismatch")));
    }

    #[tokio::test]
    async fn stale_transport_snapshot_returns_immediately_and_refreshes_once() {
        let (state, inner, reader) = blocking_transport_state(2);
        let original = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("initial transport read should succeed")
            .expect("initial transport snapshot should exist");
        age_transport_snapshot_cache(
            &state,
            PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL + Duration::from_millis(10),
        );

        let mut inactive_key = sample_key();
        inactive_key.is_active = false;
        inner
            .update_key(&inactive_key)
            .await
            .expect("provider key should update before stale refresh");

        let stale = tokio::time::timeout(
            Duration::from_millis(250),
            state.read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1"),
        )
        .await
        .expect("stale cache hit must not wait for the database refresh")
        .expect("stale transport read should succeed")
        .expect("stale transport snapshot should exist");
        assert!(Arc::ptr_eq(&original, &stale));
        assert!(stale.key.is_active);

        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("stale hit should start one background refresh");
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 2);

        for _ in 0..32 {
            let observed = state
                .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                .await
                .expect("concurrent stale read should succeed")
                .expect("concurrent stale snapshot should exist");
            assert!(Arc::ptr_eq(&original, &observed));
        }
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 2);

        reader.release_blocked_key_read.notify_one();
        wait_for_transport_refresh_to_finish(&state).await;
        let refreshed = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("refreshed transport read should succeed")
            .expect("refreshed transport snapshot should exist");
        assert!(!refreshed.key.is_active);
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn stale_transport_refresh_cannot_publish_after_generation_clear() {
        let (state, inner, reader) = blocking_transport_state(2);
        let _initial = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("initial transport read should succeed")
            .expect("initial transport snapshot should exist");
        age_transport_snapshot_cache(
            &state,
            PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL + Duration::from_millis(10),
        );

        let mut inactive_key = sample_key();
        inactive_key.is_active = false;
        inner
            .update_key(&inactive_key)
            .await
            .expect("provider key should update before stale refresh");
        let stale = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("stale transport read should succeed")
            .expect("stale transport snapshot should exist");
        assert!(stale.key.is_active);
        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("background refresh should reach the barrier");
        let cache_key = ProviderTransportSnapshotCacheKey::new("provider-1", "endpoint-1", "key-1")
            .expect("cache key should build");
        let old_inflight = state
            .provider_transport_snapshot_inflight
            .get(&cache_key)
            .expect("background refresh should own the inflight entry")
            .clone();

        inner
            .update_key(&sample_key())
            .await
            .expect("provider key should update for the new generation");
        state.clear_provider_transport_snapshot_cache();
        let current = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("new generation transport read should succeed")
            .expect("new generation snapshot should exist");
        assert!(current.key.is_active);

        reader.release_blocked_key_read.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            while Arc::strong_count(&old_inflight) > 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("old background refresh should finish after invalidation");
        let cached = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("cached new generation read should succeed")
            .expect("cached new generation snapshot should exist");
        assert!(cached.key.is_active);
        assert!(Arc::ptr_eq(&current, &cached));
    }

    #[tokio::test]
    async fn hard_expired_transport_snapshot_uses_one_synchronous_reload() {
        let (state, inner, reader) = blocking_transport_state(2);
        let _initial = state
            .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
            .await
            .expect("initial transport read should succeed")
            .expect("initial transport snapshot should exist");
        age_transport_snapshot_cache(
            &state,
            PROVIDER_TRANSPORT_SNAPSHOT_CACHE_STALE_TTL + Duration::from_secs(1),
        );
        let mut inactive_key = sample_key();
        inactive_key.is_active = false;
        inner
            .update_key(&inactive_key)
            .await
            .expect("provider key should update before hard-expiry read");

        let mut reads = Vec::new();
        for _ in 0..32 {
            let read_state = state.clone();
            reads.push(tokio::spawn(async move {
                read_state
                    .read_provider_transport_snapshot_arc("provider-1", "endpoint-1", "key-1")
                    .await
                    .expect("hard-expiry transport read should succeed")
                    .expect("hard-expiry snapshot should exist")
            }));
        }
        tokio::time::timeout(
            Duration::from_secs(1),
            reader.blocked_key_read_started.notified(),
        )
        .await
        .expect("one synchronous reload should reach the barrier");
        tokio::task::yield_now().await;
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 2);
        assert!(reads.iter().any(|read| !read.is_finished()));

        reader.release_blocked_key_read.notify_one();
        let snapshots = tokio::time::timeout(Duration::from_secs(1), async {
            let mut snapshots = Vec::with_capacity(reads.len());
            for read in reads {
                snapshots.push(read.await.expect("transport read task should join"));
            }
            snapshots
        })
        .await
        .expect("synchronous reload waiters should complete after one reload");
        assert_eq!(reader.key_reads.load(Ordering::Acquire), 2);
        let first = snapshots
            .first()
            .expect("at least one snapshot should exist");
        assert!(!first.key.is_active);
        for snapshot in snapshots.iter().skip(1) {
            assert!(Arc::ptr_eq(first, snapshot));
        }
    }

    #[test]
    fn normalizes_local_openai_refresh_token_expired_response() {
        let body = r#"{"error":{"message":"Could not validate your refresh token. Please try signing in again.","type":"invalid_request_error","param":null,"code":"refresh_token_expired"}}"#;

        assert_eq!(
            super::normalize_local_oauth_refresh_error_message(Some(401), Some(body)),
            "refresh_token 无效、已过期或已撤销，请重新登录授权"
        );
    }

    #[test]
    fn local_refresh_failure_is_appended_to_access_token_expired_marker() {
        assert_eq!(
            super::merge_local_oauth_refresh_failure_reason(
                Some("[OAUTH_EXPIRED] access token invalid"),
                "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 无效",
            ),
            Some(
                "[OAUTH_EXPIRED] access token invalid\n[REFRESH_FAILED] Token 续期失败 (401): refresh_token 无效"
                    .to_string()
            ),
        );
    }

    #[test]
    fn vertex_service_account_refresh_entry_stays_memory_only() {
        let transport = crate::provider_transport::GatewayProviderTransportSnapshot {
            provider: crate::provider_transport::snapshot::GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Vertex".to_string(),
                provider_type: "vertex_ai".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: crate::provider_transport::snapshot::GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "gemini:generate_content".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://aiplatform.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: crate::provider_transport::snapshot::GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "Gemini".to_string(),
                auth_type: "service_account".to_string(),
                is_active: true,
                api_formats: Some(vec!["gemini:generate_content".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: Some("{\"project_id\":\"demo\"}".to_string()),
            },
        };
        let entry = crate::provider_transport::CachedOAuthEntry {
            provider_type: "vertex_ai".to_string(),
            auth_header_name: "authorization".to_string(),
            auth_header_value: "Bearer access-token".to_string(),
            expires_at_unix_secs: Some(4_102_444_800),
            metadata: None,
        };

        assert!(super::local_oauth_refresh_entry_should_stay_memory_only(
            &transport, &entry
        ));
    }
}
