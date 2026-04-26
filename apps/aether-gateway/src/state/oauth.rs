use super::{
    provider_transport_snapshot_looks_refreshed, AppState, CachedProviderTransportSnapshot,
    GatewayError, ProviderTransportSnapshotCacheKey, PROVIDER_TRANSPORT_SNAPSHOT_CACHE_MAX_ENTRIES,
    PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL,
};
use crate::handlers::shared::default_provider_key_status_snapshot;
use crate::provider_transport::LocalOAuthHttpExecutor;

use super::super::provider_transport;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::read::{DeflateDecoder, GzDecoder};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Read;
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

fn oauth_invalid_reason_is_account_block(reason: Option<&str>) -> bool {
    reason
        .map(str::trim)
        .is_some_and(|value| value.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX))
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
        return None;
    }
    if current_reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX) {
        if let Some((head, _)) = current_reason.split_once("[REFRESH_FAILED]") {
            return Some(
                format!("{}\n{}", head.trim_end(), refresh_reason)
                    .trim()
                    .to_string(),
            );
        }
        return Some(format!("{current_reason}\n{refresh_reason}"));
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
        return json!({
            "code": "invalid",
            "label": "已失效",
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_invalid",
            "requires_reauth": true,
            "expiring_soon": false,
        });
    }
    if let Some(reason) = tagged_reason(invalid_reason.as_deref(), OAUTH_REFRESH_FAILED_PREFIX) {
        return json!({
            "code": "invalid",
            "label": "已失效",
            "reason": reason,
            "expires_at": expires_at_unix_secs,
            "invalid_at": invalid_at_unix_secs,
            "source": "oauth_refresh",
            "requires_reauth": true,
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
            "reason": "Token 已过期，请重新授权",
            "expires_at": expires_at_unix_secs,
            "invalid_at": Value::Null,
            "source": "expires_at",
            "requires_reauth": true,
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
        self.provider_transport_snapshot_cache
            .lock()
            .expect("provider transport snapshot cache should lock")
            .clear();
    }

    fn get_cached_provider_transport_snapshot(
        &self,
        cache_key: &ProviderTransportSnapshotCacheKey,
    ) -> Option<provider_transport::GatewayProviderTransportSnapshot> {
        let mut cache = self
            .provider_transport_snapshot_cache
            .lock()
            .expect("provider transport snapshot cache should lock");
        let cached = cache.get(cache_key).cloned()?;
        if cached.loaded_at.elapsed() <= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL {
            return Some(cached.snapshot);
        }
        cache.remove(cache_key);
        None
    }

    fn put_cached_provider_transport_snapshot(
        &self,
        cache_key: ProviderTransportSnapshotCacheKey,
        snapshot: provider_transport::GatewayProviderTransportSnapshot,
    ) {
        let mut cache = self
            .provider_transport_snapshot_cache
            .lock()
            .expect("provider transport snapshot cache should lock");
        if cache.len() >= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_MAX_ENTRIES {
            cache.retain(|_, entry| {
                entry.loaded_at.elapsed() <= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL
            });
            if cache.len() >= PROVIDER_TRANSPORT_SNAPSHOT_CACHE_MAX_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(
            cache_key,
            CachedProviderTransportSnapshot {
                loaded_at: std::time::Instant::now(),
                snapshot,
            },
        );
    }

    async fn read_provider_transport_snapshot_uncached(
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

    pub(crate) async fn read_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<crate::provider_transport::GatewayProviderTransportSnapshot>, GatewayError>
    {
        let Some(cache_key) =
            ProviderTransportSnapshotCacheKey::new(provider_id, endpoint_id, key_id)
        else {
            return self
                .read_provider_transport_snapshot_uncached(provider_id, endpoint_id, key_id)
                .await;
        };
        if let Some(snapshot) = self.get_cached_provider_transport_snapshot(&cache_key) {
            return Ok(Some(
                self.apply_global_format_conversion_override(snapshot).await,
            ));
        }

        let snapshot = self
            .read_provider_transport_snapshot_uncached(provider_id, endpoint_id, key_id)
            .await?;
        if let Some(snapshot) = snapshot.as_ref() {
            self.put_cached_provider_transport_snapshot(cache_key, snapshot.clone());
        }
        match snapshot {
            Some(snapshot) => Ok(Some(
                self.apply_global_format_conversion_override(snapshot).await,
            )),
            None => Ok(None),
        }
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

    pub(crate) async fn resolve_local_oauth_request_auth(
        &self,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
    ) -> Result<Option<provider_transport::LocalResolvedOAuthRequestAuth>, GatewayError> {
        let distributed_lock = self.data.oauth_refresh_lock_runner();
        let lock_owner = format!("aether-gateway-{}", std::process::id());
        let mut current_transport = transport.clone();
        let executor = GatewayLocalOAuthHttpExecutor { state: self };

        for _ in 0..2 {
            let resolution = match self
                .oauth_refresh
                .resolve_with_result(
                    &executor,
                    &current_transport,
                    distributed_lock.as_ref(),
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
        let distributed_lock = self.data.oauth_refresh_lock_runner();
        let lock_owner = format!("aether-gateway-admin-{}", std::process::id());
        let mut current_transport = transport.clone();
        current_transport.key.decrypted_api_key = "__placeholder__".to_string();
        let executor = GatewayLocalOAuthHttpExecutor { state: self };

        for _ in 0..2 {
            let resolution = self
                .oauth_refresh
                .force_refresh_with_result(
                    &executor,
                    &current_transport,
                    distributed_lock.as_ref(),
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
                }
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
        let updated = self
            .update_provider_catalog_key(&latest_key)
            .await?
            .is_some();
        if updated {
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

        latest_key.encrypted_api_key = encrypted_api_key;
        latest_key.encrypted_auth_config = encrypted_auth_config;
        latest_key.is_active = true;
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
        self.update_provider_catalog_key(&latest_key).await?;
        Ok(())
    }

    async fn persist_local_oauth_refresh_failure_state(
        &self,
        transport: &provider_transport::GatewayProviderTransportSnapshot,
        status_code: u16,
        body_excerpt: &str,
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
        if latest_key.oauth_invalid_reason.as_deref() == Some(merged_reason.as_str())
            && latest_key.oauth_invalid_at_unix_secs.is_some()
        {
            return Ok(false);
        }

        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        latest_key.oauth_invalid_at_unix_secs = latest_key
            .oauth_invalid_at_unix_secs
            .or(Some(now_unix_secs));
        latest_key.oauth_invalid_reason = Some(merged_reason);
        latest_key.updated_at_unix_secs = Some(now_unix_secs);
        let current_status_snapshot = latest_key.status_snapshot.take();
        latest_key.status_snapshot =
            sync_provider_key_oauth_status_snapshot(current_status_snapshot, &latest_key);

        let updated = self
            .update_provider_catalog_key(&latest_key)
            .await?
            .is_some();
        if updated {
            let _ = self.invalidate_local_oauth_refresh_entry(key_id).await;
        }
        Ok(updated)
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
        let plan = ExecutionPlan {
            request_id: request.request_id.to_string(),
            candidate_id: None,
            provider_name: Some(transport.provider.name.clone()),
            provider_id: transport.provider.id.clone(),
            endpoint_id: transport.endpoint.id.clone(),
            key_id: transport.key.id.clone(),
            method: request.method.as_str().to_string(),
            url: request.url.clone(),
            headers: request.headers.clone(),
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
            proxy: self
                .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
                .await,
            tls_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                read_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                write_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                pool_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                total_ms: Some(LOCAL_OAUTH_HTTP_TIMEOUT_MS),
                ..ExecutionTimeouts::default()
            }),
        };
        let result =
            crate::execution_runtime::execute_execution_runtime_sync_plan(self, None, &plan)
                .await
                .map_err(
                    |err| provider_transport::LocalOAuthRefreshError::InvalidResponse {
                        provider_type,
                        message: match err {
                            GatewayError::UpstreamUnavailable { message, .. }
                            | GatewayError::ControlUnavailable { message, .. }
                            | GatewayError::Internal(message) => message,
                        },
                    },
                )?;
        Ok(provider_transport::LocalOAuthHttpResponse {
            status_code: result.status_code,
            body_text: local_oauth_execution_body_text(&result),
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
    use std::sync::Arc;

    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;

    use super::AppState;
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
}
